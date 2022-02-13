mod md2tex;

use mdbook::book::BookItem;
use mdbook::config::Config as MdConfig;
use mdbook::renderer::RenderContext;
use mdbook::MDBook;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use structopt::StructOpt;

#[cfg(feature = "latex")]
use std::fmt::Write as FmtWrite;

#[cfg(feature = "pdf")]
use tectonic::status::{plain::PlainStatusBackend, ChatterLevel};
#[cfg(feature = "pdf")]
use tectonic_bridge_core::{SecuritySettings, SecurityStance};

#[derive(Debug, Clone, StructOpt)]
struct Args {
    #[structopt(
        short = "s",
        long = "standalone",
        help = "Run standalone (i.e. not as a mdbook plugin)"
    )]
    standalone: bool,
    #[structopt(help = "The book to render.", parse(from_os_str), default_value = ".")]
    root: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    pub latex: bool,
    pub pdf: bool,
    pub custom_template: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::from_args();

    let ctx: RenderContext = if args.standalone {
        let mdbook = MDBook::load(&args.root)?;

        let dest = mdbook.config.build.build_dir.clone();

        RenderContext::new(mdbook.root, mdbook.book, mdbook.config, dest)
    } else {
        let mut stdin = io::stdin();

        RenderContext::from_json(&mut stdin)?
    };

    build(&ctx)
}

fn build(ctx: &RenderContext) -> Result<(), Box<dyn std::error::Error>> {
    let cfg: Config = ctx
        .config
        .get_deserialized_opt("output.latex-pdf")?
        .unwrap();

    #[cfg(feature = "latex")]
    {
        // Read book's config values (title, authors).
        let title = ctx.config.book.title.as_ref().unwrap();
        let authors = ctx.config.book.authors.join(" \\and ");

        // Copy template data into memory.
        let mut template = if let Some(custom_template) = &cfg.custom_template {
            let mut custom_template_path = ctx.root.clone();
            custom_template_path.push(custom_template);
            std::fs::read_to_string(custom_template_path)?
        } else {
            include_str!("template.tex").to_string()
        };

        // Add title and author information.
        template = template
            .replace(r"\title{}", &format!("\\title{{{}}}", title))
            .replace(r"\author{}", &format!("\\author{{{}}}", authors));

        let mut latex = String::new();
        if cfg.latex || cfg.pdf {
            latex = get_latex(ctx, &cfg, &template);
        }

        // Output latex file.
        if cfg.latex {
            let filename = output_filename(&ctx.destination, &ctx.config, "tex");
            write_file(&latex, filename);
        }

        #[cfg(feature = "pdf")]
        {
            // Output PDF file.
            if cfg.pdf {
                let filename = output_filename(&ctx.destination, &ctx.config, "pdf");
                write_pdf(latex, filename);
            }
        }
    }

    Ok(())
}

#[cfg(feature = "latex")]
fn get_latex(ctx: &RenderContext, cfg: &Config, template: &String) -> String {
    let asset_prefix = ctx
        .destination
        .strip_prefix(&ctx.root)
        .map(|path| path.iter().map(|_| "..").collect())
        .unwrap_or_else(|_| ctx.root.clone())
        .join(ctx.config.book.src.as_path());

    let mut output_template = template.clone();

    // Iterate through markdown source.
    let mut content = String::new();

    fn for_each_chap(
        content: &mut String,
        asset_prefix: &Path,
        dest_prefix: &Path,
        cfg: &Config,
        items: &[BookItem],
    ) {
        for item in items {
            if let BookItem::Chapter(ref ch) = *item {
                let prefix = asset_prefix.join(ch.path.as_ref().and_then(|p| p.parent()).unwrap());

                let chapter_number: i32 = if let Some(number) = &ch.number {
                    number.0[0] as i32
                } else {
                    0
                };

                let latex = md2tex::Converter::new(&ch.content)
                    .dest(dest_prefix)
                    .assets(&prefix)
                    .chapter_level_offset(chapter_number)
                    .run();

                writeln!(content, "{}", latex).unwrap();

                for_each_chap(content, asset_prefix, dest_prefix, cfg, &ch.sub_items);
            }
        }
    }

    for_each_chap(
        &mut content,
        &asset_prefix,
        &ctx.destination,
        cfg,
        &ctx.book.sections,
    );

    let begin = "mdbook-latex-pdf begin";
    let target = output_template.find(&begin).unwrap() + begin.len();

    output_template.insert_str(target, &content);
    output_template
}

#[cfg(feature = "pdf")]
fn write_pdf(latex: String, filename: PathBuf) {
    // Write PDF with tectonic.
    let sb = PlainStatusBackend::new(ChatterLevel::Normal);
    let data: Vec<u8> = latex_to_pdf(&latex, sb).expect("processing failed");
    let mut output = File::create(filename).unwrap();
    output.write_all(&data).unwrap();
}

#[cfg(feature = "pdf")]
pub fn latex_to_pdf<T: AsRef<str>, S: tectonic::status::StatusBackend>(
    latex: T,
    mut status: S,
) -> anyhow::Result<Vec<u8>> {
    use tectonic::config;
    use tectonic::driver;

    let auto_create_config_file = false;
    let config = config::PersistentConfig::open(auto_create_config_file)
        .map_err(|e| anyhow::anyhow!("failed to open the default configuration file: {:?}", e))?;

    let only_cached = false;
    let bundle = config
        .default_bundle(only_cached, &mut status)
        .map_err(|e| anyhow::anyhow!("failed to load the default resource bundle: {:?}", e))?;

    let format_cache_path = config
        .format_cache_path()
        .map_err(|e| anyhow::anyhow!("failed to set up the format cache: {:?}", e))?;

    let security = SecuritySettings::new(SecurityStance::MaybeAllowInsecures);

    let mut files = {
        // Looking forward to non-lexical lifetimes!
        let mut sb = driver::ProcessingSessionBuilder::new_with_security(security);
        sb.bundle(bundle)
            .shell_escape_with_temp_dir()
            .primary_input_buffer(latex.as_ref().as_bytes())
            .tex_input_name("texput.tex")
            .format_name("latex")
            .format_cache_path(format_cache_path)
            .keep_logs(false)
            .keep_intermediates(false)
            .print_stdout(false)
            .output_format(driver::OutputFormat::Pdf)
            .do_not_write_output_files();

        let mut sess = sb.create(&mut status).map_err(|e| {
            anyhow::anyhow!("failed to initialize the LaTeX processing session: {:?}", e)
        })?;
        sess.run(&mut status)
            .map_err(|e| anyhow::anyhow!("the LaTeX engine failed: {:?}", e))?;
        sess.into_file_data()
    };

    match files.remove("texput.pdf") {
        Some(file) => Ok(file.data),
        None => Err(anyhow::anyhow!(
            "LaTeX didn't report failure, but no PDF was created (??)"
        )),
    }
}

fn write_file(data: &str, filename: PathBuf) {
    let display = filename.display();

    let mut file = match File::create(&filename) {
        Err(why) => panic!("Couldn't create {}: {}", display, why.to_string()),
        Ok(file) => file,
    };

    if let Err(why) = file.write_all(data.as_bytes()) {
        panic!("Couldn't write to {}: {}", display, why.to_string())
    }
}

fn output_filename(dest: &Path, config: &MdConfig, extension: &str) -> PathBuf {
    match config.book.title {
        Some(ref title) => dest.join(title).with_extension(extension),
        None => dest.join("book").with_extension(extension),
    }
}
