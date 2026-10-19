//! A small document tree for the profile files.
//!
//! These files are a few megabytes of plain, shallow XML with no namespaces and
//! no entities worth worrying about, so reading the whole thing into a tree is
//! simpler and quicker to work with than streaming it.

use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Default, Clone)]
pub struct Node {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    pub fn text_of(&self, name: &str) -> &str {
        self.child(name).map(|c| c.text.as_str()).unwrap_or("")
    }

    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Every node with this name, however deep.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        if self.name == name {
            out.push(self);
        }
        for child in &self.children {
            child.find_all(name, out);
        }
    }
}

pub fn parse(bytes: &[u8]) -> Option<Node> {
    // Strip a byte order mark; these files all have one.
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = false;

    let mut root: Option<Node> = None;
    let mut stack: Vec<Node> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(e)) => stack.push(open(&e)),
            Ok(Event::Empty(e)) => {
                let node = open(&e);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(node) = stack.last_mut() {
                    if let Ok(text) = e.decode() {
                        node.text.push_str(text.as_ref());
                    }
                }
            }
            Ok(Event::CData(e)) => {
                if let Some(node) = stack.last_mut() {
                    node.text
                        .push_str(&String::from_utf8_lossy(e.into_inner().as_ref()));
                }
            }
            Ok(Event::End(_)) => {
                let Some(node) = stack.pop() else { continue };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buffer.clear();
    }
    // An unclosed tail still yields what was read.
    while let Some(node) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None => root = Some(node),
        }
    }
    root
}

fn open(e: &quick_xml::events::BytesStart) -> Node {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let attributes = e
        .attributes()
        .flatten()
        .map(|a| {
            (
                String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                // WHAT: the attribute's value with &amp; and the like turned back into characters, read as XML 1.0.
                // WHY: quick-xml 0.41 renamed `unescape_value` to this (the old name now calls it); a tab or line
                // break inside a value reads as a space, as the XML standard says it should.
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
            )
        })
        .collect();
    Node {
        name,
        attributes,
        text: String::new(),
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_document_reads_into_a_tree() {
        let doc = parse(b"<a><b x='1'>hi</b><b>there</b><c/></a>").unwrap();
        assert_eq!(doc.name, "a");
        assert_eq!(doc.children.len(), 3);
        assert_eq!(doc.children_named("b").count(), 2);
        assert_eq!(doc.child("b").unwrap().text, "hi");
        assert_eq!(doc.child("b").unwrap().attribute("x"), Some("1"));
        assert_eq!(doc.text_of("b"), "hi");
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_root() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"<RevuProfile Name='x'/>");
        let doc = parse(&bytes).unwrap();
        assert_eq!(doc.name, "RevuProfile");
        assert_eq!(doc.attribute("Name"), Some("x"));
    }

    #[test]
    fn a_truncated_file_gives_back_what_it_had() {
        let doc = parse(b"<a><b>kept</b><c>").unwrap();
        assert_eq!(doc.name, "a");
        assert_eq!(doc.child("b").unwrap().text, "kept");
    }
}
