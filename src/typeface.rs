//! The family a document's body text is set in.
//!
//! The renderer reports a document's fonts by PostScript name only, so the family is inferred
//! from those names: a body face that names a serif makes the document serif, and so on.
//! Translations are then set in the matching face, so a card reads like the page it belongs to.

use crate::pdf::FontInfo;

/// The typographic family the document's body text uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Typeface {
    Serif,
    #[default]
    Sans,
    Mono,
}

/// Tokens that name a family in a PostScript name.
const SERIF: &[&str] = &[
    "serif", "slab", "times", "georgia", "garamond", "charter", "roman", "minion", "caslon", "baskerville", "didot", "antiqua", "palatino", "cambria",
    "constantia", "bookman", "bodoni", "cochin",
];
const SANS: &[&str] = &[
    "sans", "helvetica", "arial", "grotesk", "grotesque", "gothic", "franklin", "futura", "avenir", "roboto", "inter", "univers", "akkurat", "din",
    "frutiger",
];
const MONO: &[&str] = &["mono", "courier", "consol", "typewriter", "menlo"];
/// Tokens of an upright text face, and of the faces that are emphatically not one.
const BODY: &[&str] = &["regular", "book", "roman", "text", "body", "normal"];
const DISPLAY: &[&str] = &["bold", "black", "heavy", "light", "thin", "display", "headline", "title", "cond", "italic", "oblique", "caption", "script"];

fn is_body(name: &str) -> bool {
    let name = name.to_lowercase();
    BODY.iter().any(|token| name.contains(token)) && !DISPLAY.iter().any(|token| name.contains(token))
}

fn class_of(name: &str) -> Option<Typeface> {
    let name = name.to_lowercase();
    if SERIF.iter().any(|token| name.contains(token)) {
        return Some(Typeface::Serif);
    }
    if MONO.iter().any(|token| name.contains(token)) {
        return Some(Typeface::Mono);
    }
    if SANS.iter().any(|token| name.contains(token)) {
        return Some(Typeface::Sans);
    }
    None
}

/// The family a document's body text is set in. Body faces are preferred over headlines and
/// captions; among them a serif wins, because a serif body with sans display faces is common
/// and the reverse is not.
pub fn of_document(fonts: &[FontInfo]) -> Typeface {
    let body: Vec<&str> = fonts.iter().map(|font| font.name.as_str()).filter(|name| is_body(name)).collect();
    let names: Vec<&str> = if body.is_empty() { fonts.iter().map(|font| font.name.as_str()).collect() } else { body };
    let classes: Vec<Typeface> = names.iter().filter_map(|name| class_of(name)).collect();
    if classes.contains(&Typeface::Serif) {
        Typeface::Serif
    } else if classes.contains(&Typeface::Mono) {
        Typeface::Mono
    } else {
        Typeface::Sans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(name: &str) -> FontInfo {
        FontInfo { name: name.to_string(), kind: "Type1".to_string(), embedded: true, subset: true, encoding: None }
    }

    #[test]
    fn a_serif_body_beats_sans_display_faces() {
        // As in a magazine: the text is a serif, the heads and ads are not.
        let fonts = [
            font("SpiegelSerifText-Regular"),
            font("SpiegelSlab4Cd-ExtraBold"),
            font("SpiegelSans4-Regular"),
            font("GoogleSansFlex-Regular"),
            font("MDSystem-Bold"),
        ];
        assert_eq!(of_document(&fonts), Typeface::Serif);
    }

    #[test]
    fn a_sans_document_stays_sans() {
        let fonts = [font("Helvetica"), font("Roboto-Bold"), font("Inter-Regular")];
        assert_eq!(of_document(&fonts), Typeface::Sans);
    }

    #[test]
    fn monospace_body_is_mono() {
        let fonts = [font("CourierNewPSMT"), font("Courier-Bold")];
        assert_eq!(of_document(&fonts), Typeface::Mono);
    }

    #[test]
    fn unnamed_families_fall_back_to_sans() {
        let fonts = [font("MBCorpoSText-Regular"), font("MDSystem-Regular")];
        assert_eq!(of_document(&fonts), Typeface::Sans);
    }

    #[test]
    fn without_body_faces_every_font_is_considered() {
        let fonts = [font("TimesNewRomanPS-BoldMT"), font("Helvetica-Bold")];
        assert_eq!(of_document(&fonts), Typeface::Serif);
    }
}
