//! An assistant's markdown answer as widgets: text in Pango markup (bold,
//! italics, code, links, headings, lists and quotes, in the label's own font)
//! and tables as grids. Anything else keeps its text.

use gtk4::glib::markup_escape_text;
use gtk4::prelude::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Pango markup.
    Text(String),
    /// Rows of cells in Pango markup, the header first.
    Table(Vec<Vec<String>>),
}

/// `markdown` drawn: a label per run of text, a grid per table.
pub fn widget(markdown: &str, max_width_chars: i32) -> gtk4::Box {
    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    for block in to_blocks(markdown) {
        match block {
            Block::Text(markup) => {
                let label = label(&markup);
                label.set_max_width_chars(max_width_chars);
                column.append(&label);
            }
            Block::Table(rows) => column.append(&table(&rows)),
        }
    }
    column
}

fn label(markup: &str) -> gtk4::Label {
    gtk4::Label::builder()
        .label(markup)
        .use_markup(true)
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .build()
}

fn table(rows: &[Vec<String>]) -> gtk4::Grid {
    let grid = gtk4::Grid::builder()
        .column_spacing(24)
        .row_spacing(6)
        .halign(gtk4::Align::Start)
        .build();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0) as i32;
    let mut y = 0;
    for (index, row) in rows.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            let cell = label(cell);
            if index == 0 {
                cell.add_css_class("heading");
            }
            grid.attach(&cell, x as i32, y, 1, 1);
        }
        y += 1;
        if index == 0 {
            grid.attach(
                &gtk4::Separator::new(gtk4::Orientation::Horizontal),
                0,
                y,
                columns,
                1,
            );
            y += 1;
        }
    }
    grid
}

pub fn to_blocks(markdown: &str) -> Vec<Block> {
    let mut out = Markup::default();
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => out.start(tag),
            Event::End(tag) => out.end(tag),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => out.text(&text),
            Event::Code(code) => {
                out.push("<tt>");
                out.text(&code);
                out.push("</tt>");
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => out.text(&text),
            Event::FootnoteReference(name) => out.text(&format!("[{name}]")),
            Event::SoftBreak => out.push(" "),
            Event::HardBreak => out.push("\n"),
            Event::Rule => out.block(),
            Event::TaskListMarker(done) => out.push(if done { "☑ " } else { "☐ " }),
        }
    }
    out.flush();
    out.blocks
}

#[derive(Default)]
struct Markup {
    text: String,
    /// The lists open now, innermost last: the next number of an ordered one.
    lists: Vec<Option<u64>>,
    /// A list item or a quote was just opened, so its first paragraph starts
    /// right there rather than after a break.
    opened: bool,
    blocks: Vec<Block>,
    /// The table being read: its rows so far.
    rows: Option<Vec<Vec<String>>>,
}

impl Markup {
    fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.opened = false;
    }

    fn text(&mut self, s: &str) {
        self.push(&markup_escape_text(s));
    }

    /// End the run of text so far as a block of its own.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.text);
        let text = text.trim_end();
        if !text.is_empty() {
            self.blocks.push(Block::Text(text.to_string()));
        }
    }

    /// Start a block: a blank line after the one before, or a line break
    /// inside a list.
    fn block(&mut self) {
        if self.opened || self.text.is_empty() {
            return;
        }
        let trimmed = self.text.trim_end_matches('\n').len();
        self.text.truncate(trimmed);
        self.text
            .push_str(if self.lists.is_empty() { "\n\n" } else { "\n" });
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.block(),
            Tag::Heading { .. } => {
                self.block();
                self.push("<span weight=\"bold\" size=\"larger\">");
            }
            Tag::BlockQuote(_) => {
                self.block();
                self.push("<i>");
                self.opened = true;
            }
            Tag::CodeBlock(_) => {
                self.block();
                self.push("<tt>");
            }
            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.block();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                if !self.text.is_empty() && !self.text.ends_with('\n') {
                    self.push("\n");
                }
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.push(&"    ".repeat(depth));
                self.push(&marker);
                self.opened = true;
            }
            Tag::Table(_) => {
                self.flush();
                self.rows = Some(Vec::new());
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(rows) = &mut self.rows {
                    rows.push(Vec::new());
                }
            }
            Tag::Emphasis => self.push("<i>"),
            Tag::Strong => self.push("<b>"),
            Tag::Strikethrough => self.push("<s>"),
            Tag::Link { dest_url, .. } => {
                self.push(&format!("<a href=\"{}\">", markup_escape_text(&dest_url)));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => self.push("</span>"),
            TagEnd::BlockQuote(_) => self.push("</i>"),
            TagEnd::CodeBlock => {
                let trimmed = self.text.trim_end_matches('\n').len();
                self.text.truncate(trimmed);
                self.push("</tt>");
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Emphasis => self.push("</i>"),
            TagEnd::Strong => self.push("</b>"),
            TagEnd::Strikethrough => self.push("</s>"),
            TagEnd::Link => self.push("</a>"),
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.text).trim().to_string();
                if let Some(row) = self.rows.as_mut().and_then(|rows| rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some(rows) = self.rows.take() {
                    self.blocks.push(Block::Table(rows));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The markup of an answer that's one run of text.
    fn to_pango(markdown: &str) -> String {
        match &to_blocks(markdown)[..] {
            [Block::Text(markup)] => markup.clone(),
            other => panic!("expected one run of text, got {other:?}"),
        }
    }

    #[test]
    fn emphasis_strong_and_code() {
        assert_eq!(
            to_pango("Use **bold**, *italics* and `code`."),
            "Use <b>bold</b>, <i>italics</i> and <tt>code</tt>."
        );
    }

    #[test]
    fn paragraphs_are_a_blank_line_apart() {
        assert_eq!(to_pango("One.\n\nTwo."), "One.\n\nTwo.");
    }

    #[test]
    fn a_soft_break_is_a_space() {
        assert_eq!(to_pango("One\nline."), "One line.");
    }

    #[test]
    fn markup_characters_in_the_text_are_escaped() {
        assert_eq!(to_pango("1 < 2 & \"3\""), "1 &lt; 2 &amp; &quot;3&quot;");
    }

    #[test]
    fn html_is_shown_as_text() {
        assert_eq!(to_pango("a <b>tag</b>"), "a &lt;b&gt;tag&lt;/b&gt;");
    }

    #[test]
    fn a_bulleted_list() {
        assert_eq!(
            to_pango("Pick one:\n\n- tea\n- coffee"),
            "Pick one:\n\n• tea\n• coffee"
        );
    }

    #[test]
    fn a_numbered_list_counts_from_its_first_number() {
        assert_eq!(
            to_pango("3. three\n4. four\n5. five"),
            "3. three\n4. four\n5. five"
        );
    }

    #[test]
    fn a_loose_list_keeps_each_item_on_its_own_line() {
        assert_eq!(to_pango("- one\n\n- two"), "• one\n• two");
    }

    #[test]
    fn a_nested_list_is_indented() {
        assert_eq!(
            to_pango("- fruit\n  - apple\n- veg"),
            "• fruit\n    • apple\n• veg"
        );
    }

    #[test]
    fn a_heading_is_bold_and_larger() {
        assert_eq!(
            to_pango("## Pasta\n\nBoil it."),
            "<span weight=\"bold\" size=\"larger\">Pasta</span>\n\nBoil it."
        );
    }

    #[test]
    fn a_code_block_keeps_its_lines() {
        assert_eq!(
            to_pango("Run:\n\n```\ncargo build\ncargo test\n```"),
            "Run:\n\n<tt>cargo build\ncargo test</tt>"
        );
    }

    #[test]
    fn a_link_keeps_its_address() {
        assert_eq!(
            to_pango("See [the docs](https://heyari.dev/?a=1&b=2)."),
            "See <a href=\"https://heyari.dev/?a=1&amp;b=2\">the docs</a>."
        );
    }

    #[test]
    fn a_table_is_its_own_block_with_its_rows_and_cells() {
        assert_eq!(
            to_blocks(
                "Presidents:\n\n| Name | Elected |\n|---|---|\n| **Chirac** | 1995 |\n\nThat is all."
            ),
            [
                Block::Text("Presidents:".into()),
                Block::Table(vec![
                    vec!["Name".into(), "Elected".into()],
                    vec!["<b>Chirac</b>".into(), "1995".into()],
                ]),
                Block::Text("That is all.".into()),
            ]
        );
    }

    #[test]
    fn a_quote_is_in_italics() {
        assert_eq!(to_pango("> Be kind."), "<i>Be kind.</i>");
    }
}
