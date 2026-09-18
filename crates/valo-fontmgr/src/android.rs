//! Android's font configuration as the font manager.
//!
//! Android exposes no font API to a native program. It ships its fonts as a directory of
//! files and one XML file that describes them, and every text stack on the platform reads
//! that file, Skia's `SkFontMgr_android` included. It carries what the platform decides
//! and a directory scan cannot know: which family a name such as `sans-serif` means, the
//! other names that answer to it, and the order faces are tried in for a character the
//! requested family has no glyph for.
//!
//! The file lists two kinds of family. A named one is what a request for that name
//! answers with. An unnamed one is a link in the fallback chain, tagged with the
//! languages it is for, and those are tried in the order the file lists them — the order
//! the platform chose.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::files::{self, Files};
use crate::{FontManager, Slant, Style, Typeface, Watch, NORMAL_WIDTH};

/// Where the platform publishes its font configuration.
const CONFIG: &str = "/system/etc/fonts.xml";
/// Where the files the configuration names live.
const FONT_DIR: &str = "/system/fonts";
/// The family Android's user interface is set in, and what an unnamed request means.
const DEFAULT_FAMILY: &str = "sans-serif";

/// `Android` is the platform's font manager on Android.
///
/// The configuration is read once; a font file is read the first time one of its faces is
/// answered. It builds on every system, since it reads files rather than calling an API,
/// so a tool that cross-compiles for Android can read a configuration of its own with
/// [`from_config`](Android::from_config); on a system that has no such file it knows no
/// faces.
pub struct Android {
    families: Vec<Family>,
    /// Every name a request may use — a family's own and each alias — lower-cased.
    by_name: HashMap<String, Named>,
    /// The families with no name of their own, in the order the configuration lists them:
    /// the chain a character not covered by the requested family is looked for along.
    fallback: Vec<usize>,
    files: Files,
    dir: PathBuf,
}

/// One family as the configuration lists it.
struct Family {
    /// The name a request answers to, or `None` for a link in the fallback chain.
    name: Option<String>,
    /// The languages this family is for, as the configuration tags them: a BCP 47 tag
    /// such as `ja`, or `und-Arab` for a script with no language of its own.
    languages: Vec<String>,
    faces: Vec<Face>,
}

/// One face: the file it lives in, and the style the configuration gives it.
struct Face {
    file: String,
    /// The face's index in a collection file; 0 for a file holding one face.
    index: u32,
    style: Style,
}

/// What a name resolves to: a family, and the one weight an alias narrows it to.
#[derive(Clone, Copy)]
struct Named {
    family: usize,
    /// `Some` for an alias that names one weight of its target, as `sans-serif-light`
    /// names the 300 of `sans-serif`.
    weight: Option<u16>,
}

impl Android {
    /// `new` reads the platform's configuration now. It touches the file system, so read
    /// it once and keep it.
    pub fn new() -> Android {
        let config = std::fs::read_to_string(CONFIG).unwrap_or_default();
        Android::from_config(&config, PathBuf::from(FONT_DIR))
    }

    /// `from_config` reads a configuration whose files live in `dir`, for a test and for a
    /// system that keeps them elsewhere.
    pub fn from_config(xml: &str, dir: PathBuf) -> Android {
        let Config { families, aliases } = parse(xml);
        let mut by_name = HashMap::new();
        for (index, family) in families.iter().enumerate() {
            if let Some(name) = &family.name {
                by_name.insert(
                    name.to_lowercase(),
                    Named {
                        family: index,
                        weight: None,
                    },
                );
            }
        }
        // An alias names a family that the configuration lists before it, and may name
        // another alias; resolving in order therefore reaches the family either way.
        for alias in aliases {
            let Some(target) = by_name.get(&alias.to.to_lowercase()).copied() else {
                continue;
            };
            by_name.insert(
                alias.name.to_lowercase(),
                Named {
                    family: target.family,
                    weight: alias.weight.or(target.weight),
                },
            );
        }
        let fallback = families
            .iter()
            .enumerate()
            .filter(|(_, family)| family.name.is_none())
            .map(|(index, _)| index)
            .collect();
        Android {
            families,
            by_name,
            fallback,
            files: Files::default(),
            dir,
        }
    }

    /// The family a name means, with the weight an alias narrows it to.
    fn named(&self, name: &str) -> Option<Named> {
        self.by_name.get(&name.to_lowercase()).copied()
    }

    /// The faces of a family as typefaces, one per file rather than one per entry: a
    /// variable file is listed once per weight it can be set to, and those weights are the
    /// file's own to answer, not this manager's to multiply.
    fn typefaces(&mut self, named: Named) -> Vec<Typeface> {
        let family = &self.families[named.family];
        let name = family.name.clone().unwrap_or_default();
        let mut wanted: Vec<(String, u32, Style)> = Vec::new();
        for face in &family.faces {
            if named
                .weight
                .is_some_and(|weight| face.style.weight != weight)
            {
                continue;
            }
            if !wanted
                .iter()
                .any(|(file, index, _)| *file == face.file && *index == face.index)
            {
                wanted.push((face.file.clone(), face.index, face.style));
            }
        }
        wanted
            .into_iter()
            .filter_map(|(file, index, style)| self.typeface(&name, &file, index, style))
            .collect()
    }

    fn typeface(&mut self, family: &str, file: &str, index: u32, style: Style) -> Option<Typeface> {
        let data = self.files.read(&self.dir.join(file))?;
        Some(Typeface::new(data, index, family, style))
    }

    /// The families a character is looked for in, most preferred first: the requested one,
    /// then the fallback chain with the links for the preferred languages brought forward,
    /// then everything else the configuration lists.
    fn search_order(&self, family: Option<&str>, locales: &[&str]) -> Vec<usize> {
        let mut order: Vec<usize> = Vec::new();
        let push = |index: usize, order: &mut Vec<usize>| {
            if !order.contains(&index) {
                order.push(index);
            }
        };
        if let Some(named) = family.and_then(|name| self.named(name)) {
            push(named.family, &mut order);
        }
        for locale in locales {
            for &index in &self.fallback {
                if speaks(&self.families[index], locale) {
                    push(index, &mut order);
                }
            }
        }
        for &index in &self.fallback {
            push(index, &mut order);
        }
        for index in 0..self.families.len() {
            push(index, &mut order);
        }
        order
    }
}

impl Default for Android {
    fn default() -> Android {
        Android::new()
    }
}

impl FontManager for Android {
    fn family(&mut self, family: &str) -> Vec<Typeface> {
        match self.named(family) {
            Some(named) => self.typefaces(named),
            None => Vec::new(),
        }
    }

    fn match_character(
        &mut self,
        family: Option<&str>,
        style: Style,
        locales: &[&str],
        character: char,
    ) -> Option<Typeface> {
        for index in self.search_order(family, locales) {
            let named = Named {
                family: index,
                weight: None,
            };
            let mut faces = self.typefaces(named);
            // Nearest first, so the face answered is the closest one that covers.
            faces.sort_by_key(|face| face.style().distance(style));
            if let Some(covering) = faces
                .into_iter()
                .find(|face| files::covers(face.data(), face.index(), character))
            {
                return Some(covering);
            }
        }
        None
    }

    /// Android's user-interface font is the default family; the platform names no size at
    /// which it changes, as Apple's does.
    fn system_font(&mut self, _size: f32, style: Style) -> Option<Typeface> {
        crate::nearest(self.family(DEFAULT_FAMILY), style)
    }

    fn families(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .families
            .iter()
            .filter_map(|family| family.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn face_count(&self) -> usize {
        self.families.iter().map(|family| family.faces.len()).sum()
    }

    /// The platform's fonts are part of the system image and do not change while it runs.
    fn watch(&mut self, _on_change: Box<dyn Fn() + Send + Sync>) -> Option<Watch> {
        None
    }
}

/// Whether a family is for `locale`: the configuration tags a family with the language it
/// serves, and a tag covers a locale that begins with it, so `zh-Hans` covers
/// `zh-Hans-CN`. A tag for a script alone, `und-Arab`, names no language and is reached by
/// coverage instead.
fn speaks(family: &Family, locale: &str) -> bool {
    let locale = locale.to_lowercase();
    family.languages.iter().any(|language| {
        let language = language.to_lowercase();
        locale == language || locale.starts_with(&format!("{language}-"))
    })
}

/// What the configuration says, as this manager reads it.
#[derive(Default)]
struct Config {
    families: Vec<Family>,
    aliases: Vec<Alias>,
}

struct Alias {
    name: String,
    to: String,
    weight: Option<u16>,
}

/// Reads the part of the configuration this manager uses: `<family>`, its `<font>`
/// children, and `<alias>`. The rest of the file — the axis values a variable file is set
/// to, the PostScript names, which family a face is a fallback for — is skipped, because
/// the font file answers for itself once it is read.
fn parse(xml: &str) -> Config {
    let mut config = Config::default();
    let mut family: Option<Family> = None;
    for tag in Tags::new(xml) {
        match tag.name {
            "family" if tag.closing => {
                if let Some(family) = family.take() {
                    config.families.push(family);
                }
            }
            "family" => {
                if let Some(family) = family.take() {
                    config.families.push(family);
                }
                family = Some(Family {
                    name: attribute(tag.attributes, "name"),
                    languages: attribute(tag.attributes, "lang")
                        .into_iter()
                        .flat_map(|langs| {
                            langs
                                .split(',')
                                .map(str::trim)
                                .filter(|lang| !lang.is_empty())
                                .map(str::to_owned)
                                .collect::<Vec<String>>()
                        })
                        .collect(),
                    faces: Vec::new(),
                });
            }
            "font" if !tag.closing => {
                if let (Some(family), Some(face)) = (family.as_mut(), face_of(&tag)) {
                    family.faces.push(face);
                }
            }
            "alias" if !tag.closing => {
                if let (Some(name), Some(to)) = (
                    attribute(tag.attributes, "name"),
                    attribute(tag.attributes, "to"),
                ) {
                    config.aliases.push(Alias {
                        name,
                        to,
                        weight: attribute(tag.attributes, "weight")
                            .and_then(|weight| weight.parse().ok()),
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(family) = family {
        config.families.push(family);
    }
    config
}

/// A `<font>` and the file name that follows it. A face with no file names nothing and is
/// dropped.
fn face_of(tag: &Tag) -> Option<Face> {
    let file = tag.text.trim();
    if file.is_empty() {
        return None;
    }
    Some(Face {
        file: file.to_owned(),
        index: attribute(tag.attributes, "index")
            .and_then(|index| index.parse().ok())
            .unwrap_or(0),
        style: Style {
            weight: attribute(tag.attributes, "weight")
                .and_then(|weight| weight.parse().ok())
                .unwrap_or(400),
            width: NORMAL_WIDTH,
            slant: match attribute(tag.attributes, "style").as_deref() {
                Some("italic") => Slant::Italic,
                _ => Slant::Upright,
            },
        },
    })
}

/// One tag of the configuration, with the text that follows it up to the next one.
struct Tag<'a> {
    name: &'a str,
    /// Everything between the tag's name and its end, for [`attribute`] to read.
    attributes: &'a str,
    /// Whether this is a closing tag, `</family>`.
    closing: bool,
    /// The text between this tag and the next, which for a `<font>` is its file name.
    text: &'a str,
}

/// The configuration's tags in order. Android generates this file, so it is the plain
/// subset of XML the generator writes: no namespaces, no entities, no comments inside a
/// tag.
struct Tags<'a> {
    rest: &'a str,
}

impl<'a> Tags<'a> {
    fn new(xml: &'a str) -> Tags<'a> {
        Tags { rest: xml }
    }
}

impl<'a> Iterator for Tags<'a> {
    type Item = Tag<'a>;

    fn next(&mut self) -> Option<Tag<'a>> {
        let start = self.rest.find('<')?;
        let body = &self.rest[start + 1..];
        let end = body.find('>')?;
        let (body, after) = (&body[..end], &body[end + 1..]);
        self.rest = after;
        let text = match after.find('<') {
            Some(next) => &after[..next],
            None => after,
        };
        let closing = body.starts_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/');
        let name_end = body
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(body.len());
        Some(Tag {
            name: &body[..name_end],
            attributes: &body[name_end..],
            closing,
            text,
        })
    }
}

/// The value of one attribute, read attribute by attribute so that a name is never found
/// inside a longer one: `name` is not `postScriptName`.
fn attribute(attributes: &str, wanted: &str) -> Option<String> {
    let mut rest = attributes.trim_start();
    while !rest.is_empty() {
        let equals = rest.find('=')?;
        let name = rest[..equals].trim();
        let after = rest[equals + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let value_end = after[1..].find(quote)?;
        let value = &after[1..1 + value_end];
        if name == wanted {
            return Some(value.to_owned());
        }
        rest = after[2 + value_end..].trim_start();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes Android's own file uses: a variable family listed once per weight, a
    /// static family of four files, a collection face with an index, aliases with and
    /// without a weight, and fallback families tagged by language and by script.
    const CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<familyset version="23">
  <family name="sans-serif">
    <font weight="400" style="normal">
      Roboto-Regular.ttf
      <axis tag="wght" stylevalue="400"/>
    </font>
    <font weight="700" style="normal">
      Roboto-Regular.ttf
      <axis tag="wght" stylevalue="700"/>
    </font>
  </family>
  <family name="serif">
    <font weight="400" style="normal" postScriptName="NotoSerif">NotoSerif-Regular.ttf</font>
    <font weight="700" style="normal">NotoSerif-Bold.ttf</font>
    <font weight="400" style="italic">NotoSerif-Italic.ttf</font>
  </family>
  <alias name="arial" to="sans-serif"/>
  <alias name="sans-serif-light" to="sans-serif" weight="300"/>
  <family lang="ja">
    <font weight="400" style="normal" index="2" postScriptName="NotoSansCJKJP">NotoSansCJK-Regular.ttc</font>
  </family>
  <family lang="und-Arab" variant="elegant">
    <font weight="400" style="normal">NotoNaskhArabic-Regular.ttf</font>
  </family>
</familyset>
"#;

    fn config() -> Config {
        parse(CONFIG)
    }

    fn manager() -> Android {
        Android::from_config(CONFIG, PathBuf::from("/system/fonts"))
    }

    #[test]
    fn a_family_keeps_its_faces_with_their_files_and_styles() {
        let config = config();
        let serif = config
            .families
            .iter()
            .find(|family| family.name.as_deref() == Some("serif"))
            .expect("the named family");
        assert_eq!(serif.faces.len(), 3);
        assert_eq!(serif.faces[0].file, "NotoSerif-Regular.ttf");
        assert_eq!(serif.faces[0].style.weight, 400);
        assert_eq!(serif.faces[0].style.slant, Slant::Upright);
        assert_eq!(serif.faces[2].style.slant, Slant::Italic);
    }

    #[test]
    fn a_file_name_is_read_past_the_axes_that_follow_it() {
        let config = config();
        let sans = &config.families[0];
        assert_eq!(
            sans.faces.len(),
            2,
            "one entry per weight the file is set to"
        );
        for face in &sans.faces {
            assert_eq!(face.file, "Roboto-Regular.ttf");
        }
    }

    #[test]
    fn a_collection_face_keeps_its_index() {
        let config = config();
        let japanese = config
            .families
            .iter()
            .find(|family| family.languages.iter().any(|lang| lang == "ja"))
            .expect("the fallback family");
        assert_eq!(japanese.faces[0].index, 2);
        assert_eq!(japanese.faces[0].file, "NotoSansCJK-Regular.ttc");
    }

    #[test]
    fn an_alias_names_a_family_and_may_narrow_it_to_one_weight() {
        let manager = manager();
        let plain = manager.named("arial").expect("the alias");
        assert_eq!(
            manager.families[plain.family].name.as_deref(),
            Some("sans-serif")
        );
        assert_eq!(plain.weight, None);
        let light = manager.named("sans-serif-light").expect("the alias");
        assert_eq!(light.weight, Some(300));
    }

    #[test]
    fn a_name_is_matched_whatever_its_case_and_an_unknown_one_answers_nothing() {
        let mut manager = manager();
        assert!(manager.named("SANS-SERIF").is_some());
        assert!(manager.named("Arial").is_some());
        assert!(manager.named("comic sans").is_none());
        assert!(manager.family("comic sans").is_empty());
    }

    #[test]
    fn an_attribute_is_not_found_inside_a_longer_one() {
        let attributes = r#" weight="400" style="normal" postScriptName="NotoSerif""#;
        assert_eq!(attribute(attributes, "name"), None, "not postScriptName");
        assert_eq!(
            attribute(attributes, "postScriptName").as_deref(),
            Some("NotoSerif")
        );
        assert_eq!(attribute(attributes, "weight").as_deref(), Some("400"));
    }

    #[test]
    fn the_fallback_chain_is_the_unnamed_families_in_the_order_the_file_lists_them() {
        let manager = manager();
        assert_eq!(manager.fallback.len(), 2);
        for &index in &manager.fallback {
            assert!(manager.families[index].name.is_none());
        }
    }

    #[test]
    fn a_language_the_configuration_tags_brings_its_family_forward() {
        let manager = manager();
        let japanese = manager
            .fallback
            .iter()
            .copied()
            .find(|&index| speaks(&manager.families[index], "ja-JP"))
            .expect("the family for the locale");
        let order = manager.search_order(Some("sans-serif"), &["ja-JP"]);
        let sans = manager.named("sans-serif").expect("the family").family;
        assert_eq!(order[0], sans, "the requested family is tried first");
        assert_eq!(order[1], japanese, "then the chain for the locale");
    }

    #[test]
    fn a_script_tagged_family_is_reached_by_coverage_not_by_locale() {
        let manager = manager();
        let arabic = manager
            .families
            .iter()
            .find(|family| family.languages.iter().any(|lang| lang == "und-Arab"))
            .expect("the family");
        assert!(!speaks(arabic, "ar-EG"));
        assert!(manager.search_order(None, &["ar-EG"]).len() >= manager.families.len());
    }

    #[test]
    fn every_family_is_searched_even_when_no_language_matches() {
        let manager = manager();
        let order = manager.search_order(None, &[]);
        assert_eq!(order.len(), manager.families.len());
    }

    #[test]
    fn face_count_is_every_entry_the_configuration_lists() {
        assert_eq!(manager().face_count(), 7);
    }

    #[test]
    fn families_lists_the_named_ones() {
        assert_eq!(manager().families(), vec!["sans-serif", "serif"]);
    }
}
