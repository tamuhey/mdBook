use super::tokenizer::tokenize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

use pulldown_cmark::*;

use crate::book::{Book, BookItem};
use crate::config::Search;
use crate::errors::*;
use crate::theme::searcher;
use crate::utils;
use cuckoofilter::CuckooFilter;
use std::process::{Command, Stdio};
use tinysearch_shared::{Filters as _Filters, Storage};

type PostID = (String, String, String);
type Filters = _Filters<PostID>;

include!(concat!(env!("OUT_DIR"), "/engine.rs"));

pub fn run_output(cmd: &mut Command) -> Result<String, Error> {
    debug!("running {:?}", cmd);
    let output = cmd
        .stderr(Stdio::inherit())
        .output()
        .context(format!("failed to run {:?}", cmd))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        anyhow::bail!("failed to execute {:?}\nstatus: {}", cmd, output.status)
    }
}

/// Creates all files required for search.
pub fn create_files(search_config: &Search, destination: &Path, book: &Book) -> Result<()> {
    let mut index = Filters::new();

    for item in book.iter() {
        render_item(&mut index, &search_config, item)?;
    }

    let storage = Storage::from(index);
    debug!("Writing search index ✓");

    utils::fs::write_file(destination, "storage", &storage.to_bytes()?)?;
    utils::fs::write_file(destination, "searcher.js", searcher::JS)?;
    utils::fs::write_file(destination, "init.js", searcher::INIT_JS)?;
    create_search_engine(destination)?;
    if search_config.copy_js {
        utils::fs::write_file(destination, "mark.min.js", searcher::MARK_JS)?;
        debug!("Copying search files ✓");
    }

    Ok(())
}

fn extract_engine(temp_dir: &Path) -> Result<(), Error> {
    for file in FILES.file_names() {
        // This hack removes the "../" prefix that
        // gets introduced by including the crates
        // from the `bin` parent directory.
        let filepath = file.trim_start_matches("../");
        let outpath = temp_dir.join(filepath);
        if let Some(parent) = outpath.parent() {
            debug!("Creating parent dir {:?}", &parent);
            fs::create_dir_all(&parent)?;
        }
        debug!("Extracting {:?}", &outpath);
        let content = FILES.get(file)?;
        let mut outfile = File::create(&outpath)?;
        outfile.write_all(&content)?;
    }
    Ok(())
}

fn create_search_engine(destination: &Path) -> Result<(), Error> {
    FILES.set_passthrough(env::var_os("PASSTHROUGH").is_some());

    let temp_dir = tempdir()?;
    let path = temp_dir.path();
    info!("Extracting tinysearch WASM engine");
    extract_engine(path)?;
    debug!("Crate content extracted to {:?}/", &temp_dir);

    info!("Copying index into crate");
    fs::copy(destination.join("storage"), path.join("engine/storage"))?;

    info!("Compiling WASM module using wasm-pack");
    wasm_pack(&path.join("engine"), destination)?;

    info!("All done. Open the output folder with a web server to try a demo.");
    Ok(())
}

fn wasm_pack(in_dir: &Path, out_dir: &Path) -> Result<String, Error> {
    Ok(run_output(
        Command::new("wasm-pack")
            .arg("build")
            .arg(in_dir)
            .arg("--target")
            .arg("web")
            .arg("--release")
            .arg("--out-dir")
            .arg(out_dir),
    )?)
}

/// Uses the given arguments to construct a search document, then inserts it to the given index.
fn add_doc(
    index: &mut Filters,
    anchor_base: &str,
    section_id: &Option<String>,
    title: &str,
    breadcrumb: &str,
    items: &[&str],
) {
    let url = if let Some(ref id) = *section_id {
        Cow::Owned(format!("{}#{}", anchor_base, id))
    } else {
        Cow::Borrowed(anchor_base)
    };
    let url = utils::collapse_whitespace(url.trim());

    let mut words = HashSet::new();
    for item in items.into_iter().chain(&[title, breadcrumb]) {
        for word in tokenize(item) {
            words.insert(word);
        }
    }
    let mut filter = CuckooFilter::with_capacity(words.len() * 10);
    for word in words {
        filter.add(word).unwrap();
    }
    index.push((
        (title.to_owned(), url.to_string(), breadcrumb.to_owned()),
        filter,
    ));
}

/// Renders markdown into flat unformatted text and adds it to the search index.
fn render_item(index: &mut Filters, search_config: &Search, item: &BookItem) -> Result<()> {
    let chapter = match *item {
        BookItem::Chapter(ref ch) if !ch.is_draft_chapter() => ch,
        _ => return Ok(()),
    };

    let chapter_path = chapter
        .path
        .as_ref()
        .expect("Checked that path exists above");
    let filepath = Path::new(&chapter_path).with_extension("html");
    let filepath = filepath
        .to_str()
        .with_context(|| "Could not convert HTML path to str")?;
    let anchor_base = utils::fs::normalize_path(filepath);

    let mut p = utils::new_cmark_parser(&chapter.content).peekable();

    let mut in_heading = false;
    let max_section_depth = u32::from(search_config.heading_split_level);
    let mut section_id = None;
    let mut heading = String::new();
    let mut body = String::new();
    let mut breadcrumbs = chapter.parent_names.clone();
    let mut footnote_numbers = HashMap::new();
    let title = &chapter.name;

    while let Some(event) = p.next() {
        match event {
            Event::Start(Tag::Heading(i)) if i <= max_section_depth => {
                if !heading.is_empty() {
                    // Section finished, the next heading is following now
                    // Write the data to the index, and clear it for the next section
                    add_doc(
                        index,
                        &anchor_base,
                        &section_id,
                        &title,
                        &breadcrumbs.join(" » "),
                        &[&heading, &body, &title],
                    );
                    section_id = None;
                    heading.clear();
                    body.clear();
                    breadcrumbs.pop();
                }

                in_heading = true;
            }
            Event::End(Tag::Heading(i)) if i <= max_section_depth => {
                in_heading = false;
                section_id = Some(utils::id_from_content(&heading));
                breadcrumbs.push(heading.clone());
            }
            Event::Start(Tag::FootnoteDefinition(name)) => {
                let number = footnote_numbers.len() + 1;
                footnote_numbers.entry(name).or_insert(number);
            }
            Event::Html(html) => {
                let mut html_block = html.into_string();

                // As of pulldown_cmark 0.6, html events are no longer contained
                // in an HtmlBlock tag. We must collect consecutive Html events
                // into a block ourselves.
                while let Some(Event::Html(html)) = p.peek() {
                    html_block.push_str(&html);
                    p.next();
                }

                body.push_str(&clean_html(&html_block));
            }
            Event::Start(_) | Event::End(_) | Event::Rule | Event::SoftBreak | Event::HardBreak => {
                // Insert spaces where HTML output would usually seperate text
                // to ensure words don't get merged together
                if in_heading {
                    heading.push(' ');
                } else {
                    body.push(' ');
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if in_heading {
                    heading.push_str(&text);
                } else {
                    body.push_str(&text);
                }
            }
            Event::FootnoteReference(name) => {
                let len = footnote_numbers.len() + 1;
                let number = footnote_numbers.entry(name).or_insert(len);
                body.push_str(&format!(" [{}] ", number));
            }
            Event::TaskListMarker(_checked) => {}
        }
    }

    if !heading.is_empty() {
        // Make sure the last section is added to the index
        add_doc(
            index,
            &anchor_base,
            &section_id,
            &title,
            &breadcrumbs.join(" » "),
            &[&heading, &body, &title],
        );
    }

    Ok(())
}

fn clean_html(html: &str) -> String {
    lazy_static! {
        static ref AMMONIA: ammonia::Builder<'static> = {
            let mut clean_content = HashSet::new();
            clean_content.insert("script");
            clean_content.insert("style");
            let mut builder = ammonia::Builder::new();
            builder
                .tags(HashSet::new())
                .tag_attributes(HashMap::new())
                .generic_attributes(HashSet::new())
                .link_rel(None)
                .allowed_classes(HashMap::new())
                .clean_content_tags(clean_content);
            builder
        };
    }
    AMMONIA.clean(html).to_string()
}
