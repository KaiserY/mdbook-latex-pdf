mod converter;
mod events;
mod writer;

pub use converter::Converter;
use events::*;
use html5ever::driver::ParseOpts;
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use inflector::cases::kebabcase::to_kebab_case;
use markup5ever_rcdom::NodeData;
use markup5ever_rcdom::RcDom;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use regex::Regex;
use std::default::Default;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::string::String;
use tiny_skia::Pixmap;
use walkdir::WalkDir;
use writer::TexWriter;

/// Backwards-compatible function.
#[allow(dead_code)]
pub fn markdown_to_tex(content: String) -> String {
    Converter::new(&content).run()
}

/// Converts markdown string to tex string.
fn convert(converter: &Converter) -> String {
    let mut writer = TexWriter::new(String::new());

    let mut header_value = String::new();
    let mut table_buffer = TexWriter::new(String::new());

    let mut event_stack = Vec::new();

    let mut cells = 0;

    let options = Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES;

    let parser = Parser::new_ext(converter.content, options);

    let mut buffer = String::new();

    writer.new_line();

    for event in parser {
        match event {
            Event::Start(Tag::Heading(level, _, _)) => {
                let last_ev = event_stack.last().copied().unwrap_or_default();

                event_stack.push(EventType::Header);

                match level {
                    HeadingLevel::H1 => {
                        if converter.chap_offset == 0 {
                            writer.push_str(r"\chapter*{")
                        } else {
                            writer.push_str(r"\chapter{")
                        }
                    }
                    HeadingLevel::H2 => {
                        if converter.chap_offset == 0 {
                            writer.push_str(r"\section*{")
                        } else {
                            writer.push_str(r"\section{")
                        }
                    }
                    HeadingLevel::H3 => {
                        if converter.chap_offset == 0 {
                            writer.push_str(r"\subsection*{")
                        } else {
                            writer.push_str(r"\subsection{")
                        }
                    }
                    HeadingLevel::H4 => {
                        if converter.chap_offset == 0 {
                            writer.push_str(r"\subsubsection*{")
                        } else {
                            writer.push_str(r"\subsubsection{")
                        }
                    }
                    HeadingLevel::H5 => {
                        // https://tex.stackexchange.com/questions/169830/pdflatex-raise-error-when-paragraph-inside-quote-environment
                        if matches!(last_ev, EventType::BlockQuote) {
                            writer.push_str(r"\mbox{} %").new_line();
                        }
                        writer.push_str(r"\paragraph{")
                    }
                    HeadingLevel::H6 => {
                        // https://tex.stackexchange.com/questions/169830/pdflatex-raise-error-when-paragraph-inside-quote-environment
                        if matches!(last_ev, EventType::BlockQuote) {
                            writer.push_str(r"\mbox{} %").new_line();
                        }
                        writer.push_str(r"\subparagraph{")
                    }
                };
            }
            Event::End(Tag::Heading(level, _, _)) => {
                writeln!(
                    writer,
                    "}}\n\\label{{{}}}\n\\label{{{}}}",
                    header_value,
                    to_kebab_case(&header_value)
                )
                .unwrap();

                if converter.chap_offset == 0 {
                    match level {
                        HeadingLevel::H1 => {
                            writeln!(
                                writer,
                                "\\addcontentsline{{toc}}{{chapter}}{{{}}}",
                                header_value,
                            )
                            .unwrap();
                        }
                        _ => {}
                    }
                }

                if level == HeadingLevel::H5 || level == HeadingLevel::H6 {
                    writer.push_str(r"\mbox{}\\").new_line();
                }

                event_stack.pop();
            }
            Event::Start(Tag::Emphasis) => {
                event_stack.push(EventType::Emphasis);
                writer.push_str(r"\emph{");
            }
            Event::End(Tag::Emphasis) => {
                writer.push('}');
                event_stack.pop();
            }

            Event::Start(Tag::Strong) => {
                event_stack.push(EventType::Strong);
                writer.push_str(r"\textbf{");
            }
            Event::End(Tag::Strong) => {
                writer.push('}');
                event_stack.pop();
            }

            Event::Start(Tag::BlockQuote) => {
                event_stack.push(EventType::BlockQuote);
                writer.push_str(r"\begin{shadedquotation}");
            }
            Event::End(Tag::BlockQuote) => {
                writer.push_str(r"\end{shadedquotation}");
                event_stack.pop();
            }

            Event::Start(Tag::List(None)) => {
                writer.new_line().push_str(r"\begin{itemize}").new_line();
            }
            Event::End(Tag::List(None)) => {
                writer.push_str(r"\end{itemize}").new_line();
            }

            Event::Start(Tag::List(Some(_))) => {
                writer.push_str(r"\begin{enumerate}").new_line();
            }
            Event::End(Tag::List(Some(_))) => {
                writer.push_str(r"\end{enumerate}").new_line();
            }

            Event::Start(Tag::Paragraph) => {
                writer.new_line();
            }

            Event::End(Tag::Paragraph) => {
                let last_ev = event_stack.last().copied().unwrap_or_default();

                // ~ adds a space to prevent
                // "There's no line here to end" error on empty lines.
                match last_ev {
                    EventType::BlockQuote => writer.new_line(),
                    _ => writer.push_str(r"~\\").new_line(),
                };
            }

            Event::Start(Tag::Link(_, url, _)) => {
                // URL link (e.g. "https://nasa.gov/my/cool/figure.png")
                if url.starts_with("http") {
                    write!(writer, r"\href{{{}}}{{", url).unwrap();
                // local link (e.g. "my/cool/figure.png")
                } else {
                    writer.push_str(r"\hyperref[");
                    let mut found = false;

                    // iterate through `src` directory to find the resource.
                    for entry in WalkDir::new("src").into_iter().filter_map(|e| e.ok()) {
                        let _path = entry.path().to_str().unwrap();
                        let _url = &url.clone().into_string().replace("../", "");
                        if _path.ends_with(_url) {
                            match fs::File::open(_path) {
                                Ok(_) => (),
                                Err(_) => panic!("Unable to read title from {}", _path),
                            };

                            found = true;
                            break;
                        }
                    }

                    if !found {
                        writer.push_str(&*url.replace("#", ""));
                    }

                    writer.push_str("]{");
                }
            }

            Event::End(Tag::Link(_, _, _)) => {
                writer.push('}');
            }

            Event::Start(Tag::Table(_)) => {
                event_stack.push(EventType::Table);

                let table_start = [
                    r"\begingroup",
                    r"\setlength{\LTleft}{-20cm plus -1fill}",
                    r"\setlength{\LTright}{\LTleft}",
                    r"\begin{longtable}{!!!}",
                    r"\hline",
                ];

                table_buffer.new_line().push_lines(table_start).new_line();
            }

            Event::End(Tag::Table(_)) => {
                let table_end = [
                    r"\arrayrulecolor{black}\hline",
                    r"\end{longtable}",
                    r"\endgroup",
                ];

                table_buffer.push_lines(table_end).new_line();

                let mut cols = String::new();
                for _i in 0..cells {
                    write!(cols, r"|C{{{width}\textwidth}} ", width = 1. / cells as f64).unwrap();
                }

                cols.push_str("|");

                writer.push_str(&table_buffer.buffer().replace("!!!", &cols));
                table_buffer.buffer().clear();

                cells = 0;

                event_stack.pop();
            }

            Event::Start(Tag::TableHead) => {
                event_stack.push(EventType::TableHead);
            }

            Event::End(Tag::TableHead) => {
                let limit = table_buffer.buffer().len() - 2;

                table_buffer.buffer().truncate(limit);

                table_buffer
                    .back_slash()
                    .back_slash()
                    .new_line()
                    .push_str(r"\hline")
                    .new_line();

                event_stack.pop();
            }

            Event::Start(Tag::TableCell) => {
                if matches!(event_stack.last(), Some(EventType::TableHead)) {
                    table_buffer.push_str(r"\bfseries{");
                }
            }

            Event::End(Tag::TableCell) => {
                if matches!(event_stack.last(), Some(EventType::TableHead)) {
                    table_buffer.push('}');
                    cells += 1;
                }

                table_buffer.push_str(" & ");
            }

            Event::Start(Tag::TableRow) => {}

            Event::End(Tag::TableRow) => {
                let limit = table_buffer.buffer().len() - 2;

                table_buffer.buffer().truncate(limit);

                table_buffer
                    .push_str(r"\\\arrayrulecolor{lightgray}\hline")
                    .new_line();
            }

            Event::Start(Tag::Image(_, path, _title)) => {
                let mut assets_path = converter
                    .assets
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default()
                    .join(path.as_ref());

                let mut path = PathBuf::from_str(path.as_ref()).unwrap();

                // if image path ends with ".svg", run it through
                // svg2png to convert to png file.
                if path.extension().unwrap() == "svg" {
                    let img = svg2png(&assets_path);

                    path.set_extension("png");
                    let path = path
                        .strip_prefix("../..")
                        .map(Path::to_path_buf)
                        .unwrap_or(path);

                    let dest_path = converter
                        .dest
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default()
                        .join(path.clone());

                    // create output directories.
                    let _ = fs::create_dir_all(dest_path.parent().unwrap());

                    img.save_png(&dest_path).unwrap();
                    assets_path = dest_path;
                }

                writer
                    .push_str(r"\begin{figure}")
                    .new_line()
                    .push_str(r"\centering")
                    .new_line()
                    .push_str(r"\includegraphics[width=\textwidth]{")
                    .push_str(assets_path.to_string_lossy().as_ref())
                    .push('}')
                    .new_line()
                    .push_str(r"\end{figure}")
                    .new_line();
            }

            Event::Start(Tag::Item) => {
                writer.push_str(r"\item ");
            }
            Event::End(Tag::Item) => {
                writer.new_line();
            }

            Event::Start(Tag::CodeBlock(lang)) => {
                let re = Regex::new(r",.*").unwrap();

                match lang {
                    CodeBlockKind::Indented => {
                        writer.push_str(r"\begin{minted}{text}").new_line();
                    }
                    CodeBlockKind::Fenced(lang) => {
                        writer.push_str(r"\begin{minted}{");
                        let lang = re.replace(&lang, "");
                        let lang = lang.split_whitespace().next().unwrap_or_else(|| "text");

                        writeln!(writer, "{}}}", lang).unwrap();
                    }
                }

                event_stack.push(EventType::Code);
            }

            Event::End(Tag::CodeBlock(_)) => {
                writer.new_line().push_str(r"\end{minted}").new_line();

                event_stack.pop();
            }

            Event::Code(t) => {
                let wr = if event_stack
                    .iter()
                    .any(|ev| matches!(ev, EventType::Table | EventType::TableHead))
                {
                    &mut table_buffer
                } else {
                    &mut writer
                };

                if event_stack.contains(&EventType::Header) {
                    wr.push_str(r"\texttt{").escape_str(&t).push('}');
                } else {
                    let mut code = String::with_capacity(t.len());

                    if let Some((es, ee)) = converter.code_utf8_escape {
                        for c in t.chars() {
                            if c.is_ascii() {
                                code.push(c);
                            } else {
                                write!(code, "{}{}{}", es, c, ee).unwrap();
                            }
                        }
                    } else {
                        code.push_str(&*t);
                    }

                    let delims = ['|', '!', '?', '+', '@'];

                    let delim = delims
                        .iter()
                        .find(|c| !code.contains(**c))
                        .expect("Failed to find listing delmeter");

                    write!(wr, r"\lstinline{}{}{}", delim, code, delim).unwrap();
                }
            }
            Event::Html(t) => {
                let dom = parse_document(RcDom::default(), ParseOpts::default())
                    .from_utf8()
                    .read_from(&mut t.as_bytes())
                    .unwrap();

                let dom_children = &dom.document.children.borrow().to_owned();

                if dom_children.len() > 0
                    && matches!(dom_children[0].data, NodeData::Element { .. })
                {
                    let html_children = &dom_children[0].children.borrow().to_owned();

                    if html_children.len() > 1 {
                        let body_children = &html_children[1].children.borrow().to_owned();

                        if body_children.len() > 0 {
                            match &body_children[0].data {
                                NodeData::Element { name, attrs, .. } => {
                                    match name.local.as_ref() {
                                        "img" => {
                                            for attr in attrs.borrow().to_owned() {
                                                match attr.name.local.as_ref() {
                                                    "src" => {
                                                        let src_path = attr.value.to_string();

                                                        let mut assets_path = converter
                                                            .assets
                                                            .map(|p| p.to_path_buf())
                                                            .unwrap_or_default()
                                                            .join(src_path.clone());

                                                        let mut path =
                                                            PathBuf::from_str(&src_path).unwrap();

                                                        // if image path ends with ".svg", run it through
                                                        // svg2png to convert to png file.
                                                        if path.extension().unwrap() == "svg" {
                                                            let img = svg2png(&assets_path);

                                                            path.set_extension("png");
                                                            let path = path
                                                                .strip_prefix("../..")
                                                                .map(Path::to_path_buf)
                                                                .unwrap_or(path);

                                                            let dest_path = converter
                                                                .dest
                                                                .map(|p| p.to_path_buf())
                                                                .unwrap_or_default()
                                                                .join(path.clone());

                                                            // create output directories.
                                                            let _ = fs::create_dir_all(
                                                                dest_path.parent().unwrap(),
                                                            );

                                                            img.save_png(&dest_path).unwrap();
                                                            assets_path = dest_path;
                                                        }

                                                        writer
                                                            .push_str(r"\begin{figure}")
                                                            .new_line()
                                                            .push_str(r"\centering")
                                                            .new_line()
                                                            .push_str(r"\includegraphics[width=\textwidth]{")
                                                            .push_str(assets_path.to_string_lossy().as_ref())
                                                            .push('}')
                                                            .new_line()
                                                            .push_str(r"\end{figure}")
                                                            .new_line();
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Event::Text(t) => {
                // if "$$", "\[", "\(" are encountered, then begin equation
                // and don't replace any characters.

                let regex_eq_start = Regex::new(r"(\$\$|\\\[|\\\()").unwrap();
                let regex_eq_end = Regex::new(r"(\$\$|\\\]|\\\))").unwrap();

                buffer.clear();
                buffer.push_str(&t.to_string());

                let mut on_text = |wr: &mut TexWriter<String>| {
                    // TODO more elegant way to do ordered `replace`s (structs?).
                    while !buffer.is_empty() {
                        if let Some(m) = regex_eq_start.find(&buffer) {
                            let end = m.end();
                            let start = m.start();

                            wr.escape_str(&buffer[..start]).push_str(r"\[");
                            buffer.drain(..end);

                            let m = regex_eq_end
                                .find(&buffer)
                                .expect("Failed to detect end of equation");

                            let start = m.start();
                            let end = m.end();

                            wr.push_str(&buffer[..start]).push_str(r"\]");
                            buffer.drain(..end);
                        }

                        wr.escape_str(&buffer);
                        buffer.clear();

                        header_value = t.to_string();
                    }
                };

                match event_stack.last().copied().unwrap_or_default() {
                    EventType::Strong
                    | EventType::Emphasis
                    | EventType::Text
                    | EventType::Header => on_text(&mut writer),

                    EventType::Table | EventType::TableHead => on_text(&mut table_buffer),

                    _ => {
                        writer.push_str(&*t);
                    }
                }
            }

            Event::SoftBreak => {
                writer.new_line();
            }

            Event::HardBreak => {
                writer.back_slash().back_slash().new_line();
            }

            _ => (),
        }
    }

    writer.into_buffer()
}

pub fn svg2png(filename: &Path) -> Pixmap {
    let mut opt = usvg::Options::default();
    opt.resources_dir = std::fs::canonicalize(&filename)
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    opt.fontdb.load_system_fonts();
    let svg_data = std::fs::read(filename).unwrap();
    let rtree = usvg::Tree::from_data(&svg_data, &opt.to_ref()).unwrap();

    let pixmap_size = rtree.svg_node().size.to_screen_size();
    let mut pixmap = tiny_skia::Pixmap::new(pixmap_size.width(), pixmap_size.height()).unwrap();
    resvg::render(
        &rtree,
        usvg::FitTo::Original,
        tiny_skia::Transform::default(),
        pixmap.as_mut(),
    )
    .unwrap();

    pixmap
}
