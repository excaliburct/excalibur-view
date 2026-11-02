//! Steel models from IFC.
//!
//! A detailer's model is the most exact takeoff a job will ever have: every
//! beam, column, brace, plate and bolt, with its section and its weight,
//! worked out by the program that drew it. This reads that out of an IFC
//! file, whether Tekla, SDS2, Revit or anything else that writes the standard
//! made it, and totals it the way an estimator does: pieces, feet and pounds by
//! profile.
//!
//! Nothing is guessed. A weight is the one the model states, from its base
//! quantities or the exporting program's own quantity set. A part with no
//! stated weight is counted and reported as unweighed, never as weighing
//! nothing and never at a weight looked up from a table. A length is the
//! model's stated length, or failing that the straight extrusion a beam,
//! column or brace is drawn as.
//!
//! Reading is ifc-lite's parser (MPL-2.0), pinned to one version.

use std::collections::{BTreeMap, HashMap, HashSet};

use ifc_lite_core::{
    build_entity_index, AttributeValue, DecodedEntity, EntityDecoder, EntityScanner, ProjectUnits,
};

const POUNDS_PER_KG: f64 = 1.0 / 0.453_592_37;
const FEET_PER_METRE: f64 = 1.0 / 0.3048;
const INCHES_PER_METRE: f64 = 1.0 / 0.0254;

/// What a part is, as the model classes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Beam,
    Column,
    /// `IfcMember`: braces, girts, purlins, rails.
    Brace,
    Plate,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Beam => "Beam",
            Kind::Column => "Column",
            Kind::Brace => "Brace",
            Kind::Plate => "Plate",
        }
    }

    fn of(entity: &str) -> Option<Kind> {
        Some(match entity {
            "IFCBEAM" | "IFCBEAMSTANDARDCASE" => Kind::Beam,
            "IFCCOLUMN" | "IFCCOLUMNSTANDARDCASE" => Kind::Column,
            "IFCMEMBER" | "IFCMEMBERSTANDARDCASE" => Kind::Brace,
            "IFCPLATE" | "IFCPLATESTANDARDCASE" => Kind::Plate,
            _ => return None,
        })
    }
}

/// Where a length or a weight came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The standard's own quantities: `BaseQuantities`, `Qto_…BaseQuantities`.
    BaseQuantities,
    /// The exporting program's own property set: `Tekla Quantity`, `SDS2_General`.
    Exporter,
    /// The straight extrusion the part is drawn as. Lengths only.
    Shape,
    /// The model doesn't say.
    Nowhere,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::BaseQuantities => "base quantities",
            Source::Exporter => "exporter's quantities",
            Source::Shape => "its shape",
            Source::Nowhere => "not stated",
        }
    }
}

/// One piece of steel in the model.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// The entity's number in the file, `#81`.
    pub id: u32,
    pub guid: String,
    pub kind: Kind,
    /// What the model calls it: CHANNEL, VERTICAL BRACE, EMBED PLATE.
    pub name: String,
    /// The section, as the model writes it: MC6X12, W10x22, PL3/8X9.
    pub profile: String,
    /// Its own piece mark, when it has one.
    pub mark: String,
    /// The assembly it ships in, when the model groups them.
    pub assembly: String,
    /// Grade or material, as the model gives it.
    pub material: String,
    pub feet: Option<f64>,
    pub length_from: Source,
    pub pounds: Option<f64>,
    pub weight_from: Source,
}

/// One bolt, or one bolt group the exporter wrote as a single fastener.
#[derive(Clone, Debug, PartialEq)]
pub struct Bolt {
    pub id: u32,
    pub name: String,
    pub diameter_in: Option<f64>,
    pub length_in: Option<f64>,
}

impl Bolt {
    /// `3/4" x 2 1/2"`, the way a bolt is ordered.
    pub fn size(&self) -> String {
        match (self.diameter_in, self.length_in) {
            (Some(d), Some(l)) => format!("{} x {}", inches(d), inches(l)),
            (Some(d), None) => format!("{} diameter", inches(d)),
            _ => "Size not given".into(),
        }
    }
}

/// Totals for one section.
#[derive(Clone, Debug, PartialEq)]
pub struct ByProfile {
    pub kind: Kind,
    pub profile: String,
    pub pieces: usize,
    /// Of the pieces with a length.
    pub feet: f64,
    /// Of the pieces with a weight.
    pub pounds: f64,
    /// Pieces the model gave no weight for, left out of `pounds`.
    pub unweighed: usize,
    /// Pieces with no length, left out of `feet`.
    pub unmeasured: usize,
}

/// Everything read out of one IFC file.
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// `IFC2X3`, `IFC4`.
    pub schema: String,
    /// The program that wrote it, from the file's header.
    pub made_by: String,
    pub parts: Vec<Part>,
    pub bolts: Vec<Bolt>,
}

impl Model {
    /// Reads a model. `Err` only when the bytes aren't an IFC file at all; a
    /// model with odd parts in it reads, and the parts say what they lack.
    pub fn read(bytes: &[u8]) -> Result<Model, String> {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(16 * 1024)]);
        if !head.trim_start_matches('\u{feff}').trim_start().starts_with("ISO-10303-21") {
            return Err("That isn't an IFC file. It should start with ISO-10303-21.".into());
        }
        let (schema, made_by) = header(&head);
        let mut reader = Reader::new(bytes);
        let (parts, bolts) = reader.read();
        Ok(Model { schema, made_by, parts, bolts })
    }

    /// Pounds of every weighed part.
    pub fn pounds(&self) -> f64 {
        self.parts.iter().filter_map(|p| p.pounds).sum()
    }

    /// Tons of 2,000 lb.
    pub fn tons(&self) -> f64 {
        self.pounds() / 2000.0
    }

    /// Parts the model gave no weight for.
    pub fn unweighed(&self) -> usize {
        self.parts.iter().filter(|p| p.pounds.is_none()).count()
    }

    /// How many assemblies the parts ship in.
    pub fn assemblies(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| !p.assembly.is_empty())
            .map(|p| p.assembly.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    /// Pieces, feet and pounds by section, heaviest first. Sections that
    /// differ only in case (W10x22, W10X22) are one section.
    pub fn by_profile(&self) -> Vec<ByProfile> {
        let mut groups: BTreeMap<(Kind, String), ByProfile> = BTreeMap::new();
        for part in &self.parts {
            let key = (part.kind, part.profile.to_uppercase().replace(' ', ""));
            let row = groups.entry(key).or_insert_with(|| ByProfile {
                kind: part.kind,
                profile: part.profile.clone(),
                pieces: 0,
                feet: 0.0,
                pounds: 0.0,
                unweighed: 0,
                unmeasured: 0,
            });
            row.pieces += 1;
            match part.feet {
                Some(feet) => row.feet += feet,
                None => row.unmeasured += 1,
            }
            match part.pounds {
                Some(pounds) => row.pounds += pounds,
                None => row.unweighed += 1,
            }
        }
        let mut rows: Vec<ByProfile> = groups.into_values().collect();
        rows.sort_by(|a, b| {
            b.pounds
                .partial_cmp(&a.pounds)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.profile.cmp(&b.profile))
        });
        rows
    }

    /// Bolts by size, the commonest first.
    pub fn bolts_by_size(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for bolt in &self.bolts {
            *counts.entry(bolt.size()).or_default() += 1;
        }
        let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rows
    }

    /// The tonnage by section as CSV, for a spreadsheet.
    pub fn by_profile_csv(&self) -> String {
        let mut out = String::from("Kind,Profile,Pieces,Feet,Pounds,Tons,Pieces with no weight\n");
        for row in self.by_profile() {
            out.push_str(&format!(
                "{},{},{},{:.2},{:.1},{:.3},{}\n",
                row.kind.name(),
                csv(&row.profile),
                row.pieces,
                row.feet,
                row.pounds,
                row.pounds / 2000.0,
                row.unweighed
            ));
        }
        out
    }
}

impl Model {
    /// Every part as CSV: what it is, where its numbers came from.
    pub fn parts_csv(&self) -> String {
        let mut out = String::from(
            "Kind,Profile,Mark,Assembly,Name,Material,Feet,Pounds,Length from,Weight from,IFC id,GUID\n",
        );
        for p in &self.parts {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},#{},{}\n",
                p.kind.name(),
                csv(&p.profile),
                csv(&p.mark),
                csv(&p.assembly),
                csv(&p.name),
                csv(&p.material),
                p.feet.map(|f| format!("{f:.3}")).unwrap_or_default(),
                p.pounds.map(|w| format!("{w:.2}")).unwrap_or_default(),
                p.length_from.name(),
                p.weight_from.name(),
                p.id,
                csv(&p.guid)
            ));
        }
        out
    }
}

/// A whole number with commas: `14,484`.
pub fn thousands(value: f64) -> String {
    let rounded = value.round() as i64;
    let digits = rounded.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if rounded < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn csv(text: &str) -> String {
    let formula = text.starts_with(['=', '+', '-', '@', '\t', '\r']) && text.trim().parse::<f64>().is_err(); // WHAT: starts like a formula and is not a plain number. WHY: a model's names and marks are whatever its author typed, and a spreadsheet must not run them.
    let text = if formula { format!("'{text}") } else { text.to_string() }; // WHAT: a quote mark in front. WHY: read as text.
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\"")) // WHAT: quoted. WHY: one cell, whatever is in it.
    } else {
        text // WHAT: as it is. WHY: nothing to quote.
    }
}

/// Inches to the nearest sixteenth, as a fraction: `3/4"`, `2 1/2"`.
pub fn inches(value: f64) -> String {
    let sixteenths = (value * 16.0).round() as i64;
    let whole = sixteenths / 16;
    let mut num = sixteenths % 16;
    let mut den = 16;
    while num > 0 && num % 2 == 0 {
        num /= 2;
        den /= 2;
    }
    match (whole, num) {
        (w, 0) => format!("{w}\""),
        (0, n) => format!("{n}/{den}\""),
        (w, n) => format!("{w} {n}/{den}\""),
    }
}

/// `FILE_SCHEMA` and the originating system out of `FILE_NAME`.
fn header(head: &str) -> (String, String) {
    let args = |key: &str| -> Vec<String> {
        let Some(at) = head.find(key) else { return Vec::new() };
        let body = &head[at + key.len()..];
        let mut out = Vec::new();
        let (mut depth, mut quoted, mut current) = (0i32, false, String::new());
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' if quoted && chars.peek() == Some(&'\'') => {
                    current.push('\'');
                    chars.next();
                }
                '\'' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted && depth == 0 => {
                    out.push(current.trim().to_string());
                    break;
                }
                ')' if !quoted => depth -= 1,
                ',' if !quoted && depth == 0 => out.push(std::mem::take(&mut current).trim().to_string()),
                _ => current.push(c),
            }
        }
        out
    };
    let schema = args("FILE_SCHEMA(").first().cloned().unwrap_or_default();
    let made_by = args("FILE_NAME(").get(5).cloned().unwrap_or_default();
    (schema, made_by)
}

/// A property or quantity value, in the model's own units.
#[derive(Clone, Debug)]
enum Value {
    Mass(f64),
    Length(f64),
    /// A number that is neither: a count, an area, a ratio.
    Other,
    Text(String),
}

struct Reader<'a> {
    bytes: &'a [u8],
    decoder: EntityDecoder<'a>,
    kg_per_unit: f64,
    metres_per_unit: f64,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        let decoder = EntityDecoder::with_index(bytes, build_entity_index(bytes));
        Reader { bytes, decoder, kg_per_unit: 1.0, metres_per_unit: 1.0 }
    }

    fn get(&mut self, id: u32) -> Option<DecodedEntity> {
        self.decoder.decode_by_id(id).ok()
    }

    fn read(&mut self) -> (Vec<Part>, Vec<Bolt>) {
        let mut parts: Vec<(u32, Kind)> = Vec::new();
        let mut fasteners = Vec::new();
        let mut properties = Vec::new();
        let mut materials = Vec::new();
        let mut aggregates = Vec::new();
        let mut project = None;
        let mut scanner = EntityScanner::new(self.bytes);
        while let Some((id, entity, _, _)) = scanner.next_entity() {
            if let Some(kind) = Kind::of(entity) {
                parts.push((id, kind));
                continue;
            }
            match entity {
                "IFCMECHANICALFASTENER" => fasteners.push(id),
                "IFCRELDEFINESBYPROPERTIES" => properties.push(id),
                "IFCRELASSOCIATESMATERIAL" => materials.push(id),
                "IFCRELAGGREGATES" => aggregates.push(id),
                "IFCPROJECT" => project = Some(id),
                _ => {}
            }
        }
        if let Some(project) = project {
            let units = ProjectUnits::resolve(&mut self.decoder, project);
            if let Some(mass) = units.unit_for_measure("IFCMASSMEASURE") {
                self.kg_per_unit = mass.si_scale;
            }
            if let Some(length) = units.unit_for_measure("IFCLENGTHMEASURE") {
                self.metres_per_unit = length.si_scale;
            }
        }

        let wanted: HashSet<u32> = parts.iter().map(|(id, _)| *id).collect();
        let values = self.values_of(&properties, &wanted);
        let material = self.materials_of(&materials, &wanted);
        let assembly = self.assemblies_of(&aggregates, &wanted);

        let parts = parts
            .into_iter()
            .filter_map(|(id, kind)| {
                let entity = self.get(id)?;
                let empty = Vec::new();
                let own = values.get(&id).unwrap_or(&empty);
                Some(self.part(&entity, kind, own, material.get(&id), assembly.get(&id)))
            })
            .collect();
        let bolts = fasteners
            .into_iter()
            .filter_map(|id| {
                let entity = self.get(id)?;
                let size = |i: usize| {
                    entity.get_float(i).filter(|v| *v > 0.0).map(|v| v * self.metres_per_unit * INCHES_PER_METRE)
                };
                Some(Bolt {
                    id,
                    name: text(&entity, 2),
                    diameter_in: size(8),
                    length_in: size(9),
                })
            })
            .collect();
        (parts, bolts)
    }

    /// Every property and quantity attached to the parts, as
    /// (set name, property name, value), by part.
    fn values_of(
        &mut self,
        rels: &[u32],
        wanted: &HashSet<u32>,
    ) -> HashMap<u32, Vec<(String, String, Value)>> {
        let mut out: HashMap<u32, Vec<(String, String, Value)>> = HashMap::new();
        let mut sets: HashMap<u32, Vec<(String, String, Value)>> = HashMap::new();
        for rel in rels {
            let Some(rel) = self.get(*rel) else { continue };
            let objects: Vec<u32> = rel
                .get_refs(4)
                .unwrap_or_default()
                .into_iter()
                .filter(|o| wanted.contains(o))
                .collect();
            if objects.is_empty() {
                continue;
            }
            let Some(set) = rel.get_ref(5) else { continue };
            if !sets.contains_key(&set) {
                let read = self.set(set);
                sets.insert(set, read);
            }
            for object in objects {
                out.entry(object).or_default().extend(sets[&set].iter().cloned());
            }
        }
        out
    }

    fn set(&mut self, id: u32) -> Vec<(String, String, Value)> {
        let Some(set) = self.get(id) else { return Vec::new() };
        let name = text(&set, 2);
        let mut out = Vec::new();
        match set.ifc_type.as_str() {
            "IFCPROPERTYSET" => {
                for p in set.get_refs(4).unwrap_or_default() {
                    let Some(p) = self.get(p) else { continue };
                    if p.ifc_type.as_str() != "IFCPROPERTYSINGLEVALUE" {
                        continue;
                    }
                    if let Some(value) = self.typed(p.get(2)) {
                        out.push((name.clone(), text(&p, 0), value));
                    }
                }
            }
            "IFCELEMENTQUANTITY" => {
                for q in set.get_refs(5).unwrap_or_default() {
                    let Some(q) = self.get(q) else { continue };
                    let Some(number) = q.get_float(3) else { continue };
                    let value = match q.ifc_type.as_str() {
                        "IFCQUANTITYWEIGHT" => Value::Mass(number),
                        "IFCQUANTITYLENGTH" => Value::Length(number),
                        _ => Value::Other,
                    };
                    out.push((name.clone(), text(&q, 0), value));
                }
            }
            _ => {}
        }
        out
    }

    /// `IFCMASSMEASURE(27.2)` and the like.
    fn typed(&self, value: Option<&AttributeValue>) -> Option<Value> {
        let list = value?.as_list()?;
        let kind = list.first()?.as_string()?.to_ascii_uppercase();
        let inner = list.get(1)?;
        if let Some(number) = inner.as_float().or_else(|| inner.as_int().map(|i| i as f64)) {
            return Some(match kind.as_str() {
                "IFCMASSMEASURE" => Value::Mass(number),
                "IFCLENGTHMEASURE" | "IFCPOSITIVELENGTHMEASURE" | "IFCNONNEGATIVELENGTHMEASURE" => {
                    Value::Length(number)
                }
                _ => Value::Other,
            });
        }
        inner.as_string().map(|s| Value::Text(s.to_string()))
    }

    fn materials_of(&mut self, rels: &[u32], wanted: &HashSet<u32>) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        let mut names: HashMap<u32, String> = HashMap::new();
        for rel in rels {
            let Some(rel) = self.get(*rel) else { continue };
            let Some(material) = rel.get_ref(5) else { continue };
            let objects: Vec<u32> =
                rel.get_refs(4).unwrap_or_default().into_iter().filter(|o| wanted.contains(o)).collect();
            if objects.is_empty() {
                continue;
            }
            if !names.contains_key(&material) {
                let name = self.material_name(material, 0);
                names.insert(material, name);
            }
            for object in objects {
                out.insert(object, names[&material].clone());
            }
        }
        out
    }

    fn material_name(&mut self, id: u32, depth: usize) -> String {
        let Some(m) = self.get(id).filter(|_| depth < 4) else { return String::new() };
        match m.ifc_type.as_str() {
            "IFCMATERIAL" => text(&m, 0),
            // A list, a profile set, a usage: the first material in it.
            "IFCMATERIALLIST" | "IFCMATERIALPROFILESET" | "IFCMATERIALCONSTITUENTSET" => m
                .get_refs(0)
                .or_else(|| m.get_refs(2))
                .and_then(|r| r.first().copied())
                .map(|r| self.material_name(r, depth + 1))
                .unwrap_or_default(),
            "IFCMATERIALPROFILESETUSAGE" | "IFCMATERIALLAYERSETUSAGE" => {
                m.get_ref(0).map(|r| self.material_name(r, depth + 1)).unwrap_or_default()
            }
            "IFCMATERIALLAYERSET" => m
                .get_refs(0)
                .and_then(|r| r.first().copied())
                .map(|r| self.material_name(r, depth + 1))
                .unwrap_or_default(),
            "IFCMATERIALLAYER" => m.get_ref(0).map(|r| self.material_name(r, depth + 1)).unwrap_or_default(),
            "IFCMATERIALPROFILE" | "IFCMATERIALCONSTITUENT" => {
                m.get_ref(2).map(|r| self.material_name(r, depth + 1)).unwrap_or_default()
            }
            _ => String::new(),
        }
    }

    fn assemblies_of(&mut self, rels: &[u32], wanted: &HashSet<u32>) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        for rel in rels {
            let Some(rel) = self.get(*rel) else { continue };
            let Some(whole) = rel.get_ref(4) else { continue };
            let objects: Vec<u32> =
                rel.get_refs(5).unwrap_or_default().into_iter().filter(|o| wanted.contains(o)).collect();
            if objects.is_empty() {
                continue;
            }
            let Some(whole) = self.get(whole) else { continue };
            if whole.ifc_type.as_str() != "IFCELEMENTASSEMBLY" {
                continue;
            }
            let mark = [text(&whole, 7), text(&whole, 2)]
                .into_iter()
                .find(|m| !m.is_empty() && !looks_like_an_id(m))
                .unwrap_or_default();
            for object in objects {
                out.insert(object, mark.clone());
            }
        }
        out
    }

    fn part(
        &mut self,
        entity: &DecodedEntity,
        kind: Kind,
        values: &[(String, String, Value)],
        material: Option<&String>,
        assembly: Option<&String>,
    ) -> Part {
        let body = self.body(entity);
        let (kg_per_unit, metres_per_unit) = (self.kg_per_unit, self.metres_per_unit);
        let quantities = |set: &str| set == "BaseQuantities" || set.starts_with("Qto_");
        let exporter = |set: &str| !quantities(set);
        let find = |in_set: &dyn Fn(&str) -> bool, names: &[&str]| -> Option<&Value> {
            names.iter().find_map(|n| {
                values
                    .iter()
                    .find(|(set, name, _)| in_set(set) && name.eq_ignore_ascii_case(n))
                    .map(|(_, _, v)| v)
            })
        };
        let mass = |v: Option<&Value>| match v {
            Some(Value::Mass(m)) if *m > 0.0 => Some(m * kg_per_unit * POUNDS_PER_KG),
            _ => None,
        };
        let length = |v: Option<&Value>| match v {
            Some(Value::Length(l)) if *l > 0.0 => Some(l * metres_per_unit * FEET_PER_METRE),
            _ => None,
        };
        let words = |v: Option<&Value>| match v {
            Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => None,
        };

        let (pounds, weight_from) = match mass(find(&quantities, &["NetWeight", "GrossWeight"])) {
            Some(lb) => (Some(lb), Source::BaseQuantities),
            None => match mass(find(&exporter, &["Weight", "Material_Net_Weight", "Net Weight", "NetWeight"])) {
                Some(lb) => (Some(lb), Source::Exporter),
                None => (None, Source::Nowhere),
            },
        };
        let (feet, length_from) = match length(find(&quantities, &["Length"])) {
            Some(ft) => (Some(ft), Source::BaseQuantities),
            None => match length(find(&exporter, &["Length"])) {
                Some(ft) => (Some(ft), Source::Exporter),
                None => match body.as_ref().and_then(|b| b.depth).filter(|_| kind != Kind::Plate) {
                    Some(depth) => (Some(depth * metres_per_unit * FEET_PER_METRE), Source::Shape),
                    None => (None, Source::Nowhere),
                },
            },
        };

        let generic = |s: &str| {
            let upper = s.trim().to_ascii_uppercase();
            upper.is_empty()
                || matches!(
                    upper.as_str(),
                    "BEAM" | "COLUMN" | "MEMBER" | "BRACE" | "PLATE" | "NOTDEFINED" | "USERDEFINED" | "$"
                )
        };
        // The exporter's own word for it first, then the description, which
        // is where Tekla and SDS2 both write the section. The name of the
        // profile the body is swept from comes after, because SDS2 names a
        // plate's profile after its piece mark.
        let profile = [
            words(find(&exporter, &["Profile", "Cross_Section"])),
            Some(text(entity, 3)),
            body.as_ref().and_then(|b| b.profile.clone()),
            Some(text(entity, 4)),
        ]
        .into_iter()
        .flatten()
        .find(|p| !generic(p))
        .unwrap_or_else(|| "No profile given".into());

        let tag = text(entity, 7);
        let mark = [
            Some(tag),
            words(find(&exporter, &["Reference", "Material_Piecemark", "Part mark"])),
        ]
        .into_iter()
        .flatten()
        .find(|m| !m.is_empty() && !looks_like_an_id(m))
        .unwrap_or_default();
        let material = words(find(&exporter, &["Material_Grade", "Grade"]))
            .or_else(|| material.cloned())
            .unwrap_or_default();

        Part {
            id: entity.id,
            guid: text(entity, 0),
            kind,
            name: text(entity, 2),
            profile,
            mark,
            assembly: assembly.cloned().unwrap_or_default(),
            material,
            feet,
            length_from,
            pounds,
            weight_from,
        }
    }

    /// The section and the extrusion depth of a part's body, when it is drawn
    /// as a swept profile, with or without cuts taken out of it.
    fn body(&mut self, entity: &DecodedEntity) -> Option<Body> {
        let shape = self.get(entity.get_ref(6)?)?;
        let representations = shape.get_refs(2)?;
        for id in representations {
            let Some(rep) = self.get(id) else { continue };
            let identifier = text(&rep, 1);
            if !identifier.is_empty() && !identifier.eq_ignore_ascii_case("Body") {
                continue;
            }
            for item in rep.get_refs(3).unwrap_or_default() {
                if let Some(body) = self.swept(item, 0) {
                    return Some(body);
                }
            }
        }
        None
    }

    fn swept(&mut self, id: u32, depth: usize) -> Option<Body> {
        if depth > 64 {
            return None;
        }
        let item = self.get(id)?;
        match item.ifc_type.as_str() {
            "IFCEXTRUDEDAREASOLID" => {
                let profile = item.get_ref(0).and_then(|p| self.get(p)).map(|p| text(&p, 1));
                Some(Body {
                    profile: profile.filter(|p| !p.is_empty()),
                    depth: item.get_float(3).filter(|d| *d > 0.0),
                })
            }
            "IFCBOOLEANRESULT" | "IFCBOOLEANCLIPPINGRESULT" => self.swept(item.get_ref(1)?, depth + 1),
            "IFCMAPPEDITEM" => {
                let map = self.get(item.get_ref(0)?)?;
                let rep = self.get(map.get_ref(1)?)?;
                let first = *rep.get_refs(3)?.first()?;
                self.swept(first, depth + 1)
            }
            _ => None,
        }
    }
}

struct Body {
    profile: Option<String>,
    depth: Option<f64>,
}

fn text(entity: &DecodedEntity, index: usize) -> String {
    entity.get_string(index).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Tekla tags a part that has no mark of its own with `ID` and a GUID.
fn looks_like_an_id(mark: &str) -> bool {
    mark.len() > 30 && mark.starts_with("ID") && mark.matches('-').count() >= 4
}

/// Each part's shape as triangles, for drawing. See `shapes.rs`.
#[cfg(feature = "geometry")]
pub mod shapes;

#[cfg(test)]
mod a_model_written_to_a_spreadsheet {
    #[test]
    fn a_name_a_spreadsheet_would_run_is_written_as_text() {
        assert_eq!(super::csv("=HYPERLINK(\"http://x.test\")"), "\"'=HYPERLINK(\"\"http://x.test\"\")\""); // WHAT: a formula a model's author typed as a part's name. WHY: before, it ran in the spreadsheet of whoever opened the parts list.
        assert_eq!(super::csv("@SUM(A1)"), "'@SUM(A1)"); // WHAT: the same, with @. WHY: Excel reads that as a formula too.
        assert_eq!(super::csv("-3.5"), "-3.5"); // WHAT: a negative number. WHY: stays a number.
        assert_eq!(super::csv("B12"), "B12"); // WHAT: an ordinary mark. WHY: left alone.
    }
}
