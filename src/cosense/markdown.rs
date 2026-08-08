//! Convert a Cosense page (line-based text) into Markdown.
//!
//! The API returns pages as a list of lines. They are rejoined and parsed as one
//! document so `cosy::parse()` can recognise multi-line blocks (`code:`,
//! `table:`), and the resulting AST is walked here to emit Markdown.

use std::fmt::Write as _;

use cosy::ast::{Block, BlockContent, CodeBlockMeta, Link, Node, ShellPrompt};

pub fn render(project: &str, lines: &[String]) -> String {
    let input = lines.join("\n");
    let Ok(doc) = cosy::parse(&input, &()) else {
        // Raw source is the least-bad fallback, but the caller hands it to the
        // model under `body_markdown`, where bracket syntax reads as Markdown.
        // Silent, this would be indistinguishable from success.
        tracing::warn!(
            project,
            bytes = input.len(),
            "failed to parse Cosense page; returning raw source as Markdown"
        );
        return input;
    };

    let mut out = String::new();
    for block in &doc.0 {
        render_block(block, project, &mut out);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

fn render_block(block: &Block<()>, project: &str, out: &mut String) {
    let indent = block.indent;
    match &block.content {
        BlockContent::Line(nodes) => {
            if let Some(level) = detect_heading_level(nodes) {
                out.push_str(&"#".repeat(level));
                out.push(' ');
                // Found by search, not by index: detect_heading_level tolerates
                // surrounding whitespace nodes, so the decoration is not
                // necessarily first.
                if let Some(Node::Decoration { nodes: inner, .. }) =
                    nodes.iter().find(|n| matches!(n, Node::Decoration { .. }))
                {
                    render_nodes(inner, project, out);
                }
                return;
            }

            if indent > 0 {
                out.push_str(&"  ".repeat(indent - 1));
                out.push_str("- ");
            }
            render_nodes(nodes, project, out);
        }
        BlockContent::CodeBlock { meta, content } => {
            // NOTE: a block with `indent > 0` was nested under a list item;
            // emitting at column 0 loses that. Accepted — nested code blocks are
            // rare in practice.
            let (filename, lang) = code_block_label(meta);
            if let Some(f) = filename {
                writeln!(out, "**{f}**").unwrap();
            }
            let trailing = if content.ends_with('\n') { "" } else { "\n" };
            let fence = "`".repeat(fence_width(content));
            write!(out, "{fence}{lang}\n{content}{trailing}{fence}").unwrap();
        }
        BlockContent::Table { name, rows } => {
            render_table(name, rows, project, out);
        }
        BlockContent::Quote(nodes) => {
            out.push_str("> ");
            render_nodes(nodes, project, out);
        }
        BlockContent::Helpfeel(s) => write!(out, "> ? {s}").unwrap(),
        BlockContent::CommandLine { prompt, command } => {
            let p = match prompt {
                ShellPrompt::Percent => '%',
                _ => '$',
            };
            write!(out, "```sh\n{p} {command}\n```").unwrap();
        }
        // Custom blocks carry no payload in this parser configuration, and a
        // future variant is better skipped than rendered as garbage.
        _ => {}
    }
}

/// Number of backticks needed to fence `content` without it breaking out.
///
/// A page documenting Markdown holds code blocks containing fences; with a fixed
/// three-backtick fence the inner one closes the outer and the rest of the body
/// escapes as prose. `CommonMark` fixes this by letting the outer fence be
/// longer than any run inside.
fn fence_width(content: &str) -> usize {
    let longest_run = content.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    longest_run.saturating_add(1).max(3)
}

/// Asterisk count that Cosense pages conventionally use for a top-level
/// heading, and which therefore maps to `#`.
const COSENSE_TOP_HEADING_STARS: usize = 3;

/// Markdown heading depth for a Cosense size decoration.
///
/// The two scales run in opposite directions — more asterisks means larger text,
/// a larger Markdown number means a smaller heading — so the count cannot be
/// used as a depth directly. Counts above the top level clamp to `#`.
fn heading_level(stars: usize) -> usize {
    (COSENSE_TOP_HEADING_STARS + 1)
        .saturating_sub(stars)
        .clamp(1, 6)
}

fn detect_heading_level(nodes: &[Node<()>]) -> Option<usize> {
    // A line is a heading when its only content is one `[* x]`-style size
    // decoration. Trailing whitespace is common in hand-edited pages and must
    // not demote it to bold.
    let mut significant = nodes.iter().filter(|n| !is_blank_text(n));
    let first = significant.next()?;
    if significant.next().is_some() {
        return None;
    }
    if let Node::Decoration { decos, .. } = first
        && !decos.is_empty()
        && decos.chars().all(|c| c == '*')
    {
        return Some(heading_level(decos.len()));
    }
    None
}

fn is_blank_text(node: &Node<()>) -> bool {
    matches!(node, Node::Text(s) if s.trim().is_empty())
}

fn render_nodes(nodes: &[Node<()>], project: &str, out: &mut String) {
    for n in nodes {
        render_node(n, project, out);
    }
}

fn render_node(node: &Node<()>, project: &str, out: &mut String) {
    match node {
        Node::Text(s) => out.push_str(s),
        Node::Link(l) => render_link(l, project, out),
        Node::Image(url) => write!(out, "![]({url})").unwrap(),
        Node::LinkedImage { src, href } => write!(out, "[![]({src})]({href})").unwrap(),
        Node::Icon { name, count } => {
            for _ in 0..(*count).max(1) {
                write!(out, ":{name}:").unwrap();
            }
        }
        Node::InlineCode(s) => write!(out, "`{s}`").unwrap(),
        Node::Math(s) => write!(out, "${s}$").unwrap(),
        Node::Hashtag(s) => write!(out, "#{s}").unwrap(),
        Node::Decoration { decos, nodes } => {
            let (open, close) = decoration_marks(decos);
            out.push_str(&open);
            render_nodes(nodes, project, out);
            out.push_str(&close);
        }
        Node::Strong(nodes) => {
            out.push_str("**");
            render_nodes(nodes, project, out);
            out.push_str("**");
        }
        Node::Coordinate {
            latitude,
            longitude,
            zoom,
        } => {
            use cosy::ast::{Latitude, Longitude};
            let (lat_dir, lat_v) = match latitude {
                Latitude::North(v) => ('N', *v),
                Latitude::South(v) => ('S', *v),
            };
            let (lon_dir, lon_v) = match longitude {
                Longitude::East(v) => ('E', *v),
                Longitude::West(v) => ('W', *v),
            };
            match zoom {
                Some(z) => write!(out, "({lat_dir}{lat_v},{lon_dir}{lon_v},Z{z})").unwrap(),
                None => write!(out, "({lat_dir}{lat_v},{lon_dir}{lon_v})").unwrap(),
            }
        }
        // Skipped for the same reason as in render_block.
        _ => {}
    }
}

fn decoration_marks(decos: &str) -> (String, String) {
    // Cosense decorations stack: "*-/" is bold + strike + italic. Chars the
    // parser permits but Cosense gives no visual rule are ignored; the wrapped
    // text is still emitted, so no information is dropped.
    let mut open = String::new();
    let mut close = String::new();
    let stack = [
        ('_', "<u>", "</u>"),
        ('-', "~~", "~~"),
        ('*', "**", "**"),
        ('/', "*", "*"),
    ];
    for (ch, o, c) in stack {
        if decos.contains(ch) {
            open.push_str(o);
            close.insert_str(0, c);
        }
    }
    (open, close)
}

fn code_block_label(meta: &CodeBlockMeta) -> (Option<&str>, &str) {
    match meta {
        CodeBlockMeta::NameOrLang(s) => {
            // `code:foo` — bare language id; `code:foo.ext` — filename.
            if s.contains('.') {
                (Some(s.as_str()), infer_lang(s))
            } else {
                (None, s.as_str())
            }
        }
        CodeBlockMeta::Both { filename, language } => (Some(filename.as_str()), language.as_str()),
        // `None`, plus any variant added later: no filename and no language.
        _ => (None, ""),
    }
}

fn render_link(link: &Link<()>, project: &str, out: &mut String) {
    match link {
        Link::Page(title) => {
            write!(
                out,
                "[{title}](https://scrapbox.io/{}/{})",
                urlencoding::encode(project),
                urlencoding::encode(title),
            )
            .unwrap();
        }
        Link::Url(url) => write!(out, "<{url}>").unwrap(),
        Link::WithLabel { href, label } => {
            out.push('[');
            render_nodes(label, project, out);
            write!(out, "]({href})").unwrap();
        }
        Link::Project(p) => {
            write!(out, "[{p}](https://scrapbox.io/{})", urlencoding::encode(p)).unwrap();
        }
        Link::ProjectPage { project: p, page } => {
            write!(
                out,
                "[{p}/{page}](https://scrapbox.io/{}/{})",
                urlencoding::encode(p),
                urlencoding::encode(page),
            )
            .unwrap();
        }
        _ => {}
    }
}

fn render_table(name: &str, rows: &[Vec<Vec<Node<()>>>], project: &str, out: &mut String) {
    writeln!(out, "**{name}**").unwrap();
    if rows.is_empty() {
        return;
    }
    let max_cols = rows.iter().map(std::vec::Vec::len).max().unwrap_or(0);
    if max_cols == 0 {
        return;
    }
    write_row(&rows[0], max_cols, project, out);
    out.push('\n');
    out.push('|');
    for _ in 0..max_cols {
        out.push_str(" --- |");
    }
    for row in rows.iter().skip(1) {
        out.push('\n');
        write_row(row, max_cols, project, out);
    }
}

fn write_row(row: &[Vec<Node<()>>], max_cols: usize, project: &str, out: &mut String) {
    out.push('|');
    for i in 0..max_cols {
        out.push(' ');
        if let Some(cell) = row.get(i) {
            let mut rendered = String::new();
            render_nodes(cell, project, &mut rendered);
            out.push_str(&escape_table_cell(&rendered));
        }
        out.push_str(" |");
    }
}

/// Make a rendered cell safe to place between pipes.
///
/// Cells hold prose, and prose contains `|` (shell pipelines, `a|b`
/// alternatives). Left raw it adds a column, so the row stops lining up with
/// the separator. Newlines end the row outright, so they collapse to spaces.
fn escape_table_cell(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    for ch in cell.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn infer_lang(name_or_lang: &str) -> &str {
    if let Some(idx) = name_or_lang.rfind('.') {
        &name_or_lang[idx + 1..]
    } else {
        name_or_lang
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(input: &str) -> String {
        let lines: Vec<String> = input.lines().map(str::to_owned).collect();
        render("proj", &lines)
    }

    #[test]
    fn plain_line_passes_through() {
        assert_eq!(r("hello world"), "hello world\n");
    }

    #[test]
    fn headings_use_size_decoration() {
        // Inverted scales: the smallest decoration is the deepest heading.
        assert_eq!(r("[* smallest]"), "### smallest\n");
        assert_eq!(r("[** middle]"), "## middle\n");
        assert_eq!(r("[*** largest]"), "# largest\n");
    }

    #[test]
    fn headings_above_the_top_level_clamp_to_h1() {
        assert_eq!(r("[**** x]"), "# x\n");
        assert_eq!(r("[********** x]"), "# x\n");
    }

    #[test]
    fn heading_survives_trailing_whitespace() {
        assert_eq!(r("[* h] "), "### h\n");
    }

    #[test]
    fn heading_requires_the_decoration_to_be_the_whole_line() {
        assert!(!r("[* h] and more").starts_with('#'));
        assert!(!r("before [* h]").starts_with('#'));
    }

    #[test]
    fn code_block_fence_grows_past_backticks_in_the_body() {
        let out = r("code:x.md\n ```\n inner\n ```");
        assert!(
            out.contains("````md"),
            "fence must outgrow the inner run: {out}"
        );
        let body_start = out.find("````md").unwrap();
        assert!(
            out[body_start..].trim_end().ends_with("````"),
            "block must close with the same fence: {out}"
        );
    }

    #[test]
    fn table_cells_escape_pipes() {
        let out = r("table:T\n a|b\tc");
        assert!(
            out.contains(r"a\|b"),
            "an unescaped pipe adds a column: {out}"
        );
    }

    #[test]
    fn page_link_renders_with_project_url() {
        let out = r("see [Some Page] for details");
        assert!(
            out.contains("[Some Page](https://scrapbox.io/proj/Some%20Page)"),
            "got: {out}"
        );
    }

    #[test]
    fn url_with_label() {
        let out = r("[https://example.com Example]");
        assert!(out.contains("[Example](https://example.com"), "got: {out}");
    }

    #[test]
    fn bare_url_in_brackets_renders_as_autolink() {
        let out = r("[https://example.com]");
        assert!(out.contains("<https://example.com"), "got: {out}");
    }

    #[test]
    fn indented_lines_become_bullet_list() {
        let out = r(" first\n  second");
        assert!(out.contains("- first"), "got: {out}");
        assert!(out.contains("  - second"), "got: {out}");
    }

    #[test]
    fn code_block_with_filename_uses_extension_as_lang() {
        let input = "code:main.rs\n fn main() {}\n";
        let out = r(input);
        assert!(out.contains("```rs"), "expected opening ```rs in: {out}");
        assert!(out.contains("fn main()"), "expected body content in: {out}");
        assert_eq!(
            out.matches("```").count(),
            2,
            "expected exactly 2 fences in: {out}"
        );
    }

    #[test]
    fn code_block_without_metadata_emits_blank_lang() {
        let input = "code:\n some content\n";
        let out = r(input);
        assert!(out.contains("```\n") || out.contains("```"), "got: {out}");
        assert_eq!(
            out.matches("```").count(),
            2,
            "expected exactly 2 fences in: {out}"
        );
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(render("p", &[]), "");
    }

    #[test]
    fn table_renders_with_pipe_syntax() {
        // Cosense table: `table:Name` followed by space-indented, tab-separated rows.
        let input = "table:Stats\n  header1\theader2\n  row1a\trow1b\n";
        let out = r(input);
        assert!(
            out.contains("**Stats**"),
            "expected table name as bold in: {out}"
        );
        assert!(out.contains('|'), "expected pipe syntax in: {out}");
        assert!(out.contains("---"), "expected separator row in: {out}");
    }

    #[test]
    fn strikethrough_decoration() {
        let out = r("[- gone]");
        assert!(out.contains("~~gone~~"), "got: {out}");
    }

    #[test]
    fn bold_italic_combination() {
        let out = r("[*/ emph]");
        assert!(out.contains("***emph***"), "got: {out}");
    }

    #[test]
    fn underline_decoration_preserved() {
        let out = r("[_ note]");
        assert!(out.contains("<u>note</u>"), "got: {out}");
    }

    #[test]
    fn three_decorations_stack() {
        let out = r("[*-/ all]");
        assert!(out.contains("**"), "missing bold in: {out}");
        assert!(
            out.contains("*all*") || out.contains("*all"),
            "missing italic in: {out}"
        );
        assert!(
            out.contains("~~all~~") || out.contains("~~"),
            "missing strike in: {out}"
        );
    }

    #[test]
    fn coordinate_zoom_preserved() {
        let out = r("[N35.65,E139.74,Z14]");
        assert!(out.contains("Z14"), "expected zoom in: {out}");
    }

    #[test]
    fn coordinate_without_zoom() {
        let out = r("[N35.65,E139.74]");
        assert!(
            out.contains("N35.65") && out.contains("E139.74"),
            "got: {out}"
        );
        assert!(!out.contains('Z'), "unexpected Z in: {out}");
    }

    #[test]
    fn code_block_with_filename_and_language_preserves_both() {
        let input = "code:main.rs(rust)\n fn main() {}\n";
        let out = r(input);
        assert!(
            out.contains("**main.rs**"),
            "expected filename label in: {out}"
        );
        assert!(out.contains("```rust"), "expected language fence in: {out}");
    }

    #[test]
    fn code_block_with_bare_language_no_filename_label() {
        let input = "code:rust\n fn main() {}\n";
        let out = r(input);
        assert!(out.contains("```rust"), "expected language fence in: {out}");
        assert!(
            !out.contains("**rust**"),
            "should not emit filename label in: {out}"
        );
    }

    #[test]
    fn heading_detection_only_when_decoration_is_pure_stars() {
        // [*- x] = bold + strike, NOT a heading
        let out = r("[*- x]");
        assert!(!out.starts_with("# "), "got: {out}");
    }

    #[test]
    fn infer_lang_extracts_extension() {
        assert_eq!(infer_lang("main.rs"), "rs");
        assert_eq!(infer_lang("rust"), "rust");
        assert_eq!(infer_lang("foo.tar.gz"), "gz");
    }
}
