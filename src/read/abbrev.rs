//! Functions for parsing DWARF debugging abbreviations.

use crate::collections::btree_map;
use crate::vec::Vec;

use crate::common::DebugAbbrevOffset;
use crate::constants;
use crate::endianity::Endianity;
use crate::read::{EndianSlice, Error, Reader, Result, Section, UnitHeader};

/// The `DebugAbbrev` struct represents the abbreviations describing
/// `DebuggingInformationEntry`s' attribute names and forms found in the
/// `.debug_abbrev` section.
#[derive(Debug, Default, Clone, Copy)]
pub struct DebugAbbrev<R: Reader> {
    debug_abbrev_section: R,
}

impl<'input, Endian> DebugAbbrev<EndianSlice<'input, Endian>>
where
    Endian: Endianity,
{
    /// Construct a new `DebugAbbrev` instance from the data in the `.debug_abbrev`
    /// section.
    ///
    /// It is the caller's responsibility to read the `.debug_abbrev` section and
    /// present it as a `&[u8]` slice. That means using some ELF loader on
    /// Linux, a Mach-O loader on OSX, etc.
    ///
    /// ```
    /// use gimli::{DebugAbbrev, LittleEndian};
    ///
    /// # let buf = [0x00, 0x01, 0x02, 0x03];
    /// # let read_debug_abbrev_section_somehow = || &buf;
    /// let debug_abbrev = DebugAbbrev::new(read_debug_abbrev_section_somehow(), LittleEndian);
    /// ```
    pub fn new(debug_abbrev_section: &'input [u8], endian: Endian) -> Self {
        Self::from(EndianSlice::new(debug_abbrev_section, endian))
    }
}

impl<R: Reader> DebugAbbrev<R> {
    /// Parse the abbreviations at the given `offset` within this
    /// `.debug_abbrev` section.
    ///
    /// The `offset` should generally be retrieved from a unit header.
    pub fn abbreviations(
        &self,
        debug_abbrev_offset: DebugAbbrevOffset<R::Offset>,
    ) -> Result<Abbreviations> {
        let input = &mut self.debug_abbrev_section.clone();
        input.skip(debug_abbrev_offset.0)?;
        Abbreviations::parse(input)
    }
}

impl<R: Reader> Section<R> for DebugAbbrev<R> {
    fn section_name() -> &'static str {
        ".debug_abbrev"
    }
}

impl<R: Reader> From<R> for DebugAbbrev<R> {
    fn from(debug_abbrev_section: R) -> Self {
        DebugAbbrev {
            debug_abbrev_section,
        }
    }
}

/// A set of type abbreviations.
///
/// Construct an `Abbreviations` instance with the
/// [`abbreviations()`](struct.UnitHeader.html#method.abbreviations)
/// method.
#[derive(Debug, Default, Clone)]
pub struct Abbreviations {
    vec: Vec<Abbreviation>,
    map: btree_map::BTreeMap<u64, Abbreviation>,
}

impl Abbreviations {
    /// Construct a new, empty set of abbreviations.
    fn empty() -> Abbreviations {
        Abbreviations {
            vec: Vec::new(),
            map: btree_map::BTreeMap::new(),
        }
    }

    /// Insert an abbreviation into the set.
    ///
    /// Returns `Ok` if it is the first abbreviation in the set with its code,
    /// `Err` if the code is a duplicate and there already exists an
    /// abbreviation in the set with the given abbreviation's code.
    fn insert(&mut self, abbrev: Abbreviation) -> ::std::result::Result<(), ()> {
        let code_usize = abbrev.code as usize;
        if code_usize as u64 == abbrev.code {
            // Optimize for sequential abbreviation codes by storing them
            // in a Vec, as long as the map doesn't already contain them.
            // A potential further optimization would be to allow some
            // holes in the Vec, but there's no need for that yet.
            if code_usize - 1 < self.vec.len() {
                return Err(());
            } else if code_usize - 1 == self.vec.len() {
                if !self.map.is_empty() && self.map.contains_key(&abbrev.code) {
                    return Err(());
                } else {
                    self.vec.push(abbrev);
                    return Ok(());
                }
            }
        }
        match self.map.entry(abbrev.code) {
            btree_map::Entry::Occupied(_) => Err(()),
            btree_map::Entry::Vacant(entry) => {
                entry.insert(abbrev);
                Ok(())
            }
        }
    }

    /// Get the abbreviation associated with the given code.
    #[inline]
    pub fn get(&self, code: u64) -> Option<&Abbreviation> {
        let code_usize = code as usize;
        if code_usize as u64 == code && code_usize - 1 < self.vec.len() {
            Some(&self.vec[code_usize - 1])
        } else {
            self.map.get(&code)
        }
    }

    /// Parse a series of abbreviations, terminated by a null abbreviation.
    fn parse<R: Reader>(input: &mut R) -> Result<Abbreviations> {
        let mut abbrevs = Abbreviations::empty();

        while let Some(abbrev) = Abbreviation::parse(input)? {
            if abbrevs.insert(abbrev).is_err() {
                return Err(Error::DuplicateAbbreviationCode);
            }
        }

        Ok(abbrevs)
    }
}

/// An abbreviation describes the shape of a `DebuggingInformationEntry`'s type:
/// its code, tag type, whether it has children, and its set of attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abbreviation {
    code: u64,
    tag: constants::DwTag,
    has_children: constants::DwChildren,
    attributes: Vec<AttributeSpecification>,
}

impl Abbreviation {
    /// Construct a new `Abbreviation`.
    ///
    /// ### Panics
    ///
    /// Panics if `code` is `0`.
    pub fn new(
        code: u64,
        tag: constants::DwTag,
        has_children: constants::DwChildren,
        attributes: Vec<AttributeSpecification>,
    ) -> Abbreviation {
        assert_ne!(code, 0);
        Abbreviation {
            code,
            tag,
            has_children,
            attributes,
        }
    }

    /// Get this abbreviation's code.
    #[inline]
    pub fn code(&self) -> u64 {
        self.code
    }

    /// Get this abbreviation's tag.
    #[inline]
    pub fn tag(&self) -> constants::DwTag {
        self.tag
    }

    /// Return true if this abbreviation's type has children, false otherwise.
    #[inline]
    pub fn has_children(&self) -> bool {
        self.has_children == constants::DW_CHILDREN_yes
    }

    /// Get this abbreviation's attributes.
    #[inline]
    pub fn attributes(&self) -> &[AttributeSpecification] {
        &self.attributes[..]
    }

    /// Parse an abbreviation's tag.
    fn parse_tag<R: Reader>(input: &mut R) -> Result<constants::DwTag> {
        let val = input.read_uleb128()?;
        if val == 0 {
            Err(Error::AbbreviationTagZero)
        } else {
            Ok(constants::DwTag(val))
        }
    }

    /// Parse an abbreviation's "does the type have children?" byte.
    fn parse_has_children<R: Reader>(input: &mut R) -> Result<constants::DwChildren> {
        let val = input.read_u8()?;
        let val = constants::DwChildren(val);
        if val == constants::DW_CHILDREN_no || val == constants::DW_CHILDREN_yes {
            Ok(val)
        } else {
            Err(Error::BadHasChildren)
        }
    }

    /// Parse a series of attribute specifications, terminated by a null attribute
    /// specification.
    fn parse_attributes<R: Reader>(input: &mut R) -> Result<Vec<AttributeSpecification>> {
        let mut attrs = Vec::new();

        while let Some(attr) = AttributeSpecification::parse(input)? {
            attrs.push(attr);
        }

        Ok(attrs)
    }

    /// Parse an abbreviation. Return `None` for the null abbreviation, `Some`
    /// for an actual abbreviation.
    fn parse<R: Reader>(input: &mut R) -> Result<Option<Abbreviation>> {
        let code = input.read_uleb128()?;
        if code == 0 {
            return Ok(None);
        }

        let tag = Self::parse_tag(input)?;
        let has_children = Self::parse_has_children(input)?;
        let attributes = Self::parse_attributes(input)?;
        let abbrev = Abbreviation::new(code, tag, has_children, attributes);
        Ok(Some(abbrev))
    }
}

/// The description of an attribute in an abbreviated type. It is a pair of name
/// and form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeSpecification {
    name: constants::DwAt,
    form: constants::DwForm,
    implicit_const_value: i64,
}

impl AttributeSpecification {
    /// Construct a new `AttributeSpecification` from the given name and form
    /// and implicit const value.
    #[inline]
    pub fn new(
        name: constants::DwAt,
        form: constants::DwForm,
        implicit_const_value: Option<i64>,
    ) -> AttributeSpecification {
        debug_assert!(
            (form == constants::DW_FORM_implicit_const && implicit_const_value.is_some())
                || (form != constants::DW_FORM_implicit_const && implicit_const_value.is_none())
        );
        AttributeSpecification {
            name,
            form,
            implicit_const_value: implicit_const_value.unwrap_or(0),
        }
    }

    /// Get the attribute's name.
    #[inline]
    pub fn name(&self) -> constants::DwAt {
        self.name
    }

    /// Get the attribute's form.
    #[inline]
    pub fn form(&self) -> constants::DwForm {
        self.form
    }

    /// Get the attribute's implicit const value.
    #[inline]
    pub fn implicit_const_value(&self) -> i64 {
        assert!(self.form == constants::DW_FORM_implicit_const);
        self.implicit_const_value
    }

    /// Return the size of the attribute, in bytes.
    ///
    /// Note that because some attributes are variably sized, the size cannot
    /// always be known without parsing, in which case we return `None`.
    pub fn size<R: Reader>(&self, header: &UnitHeader<R, R::Offset>) -> Option<usize> {
        match self.form {
            constants::DW_FORM_addr => Some(header.address_size() as usize),

            constants::DW_FORM_implicit_const => Some(0),

            constants::DW_FORM_flag |
            constants::DW_FORM_flag_present |
            constants::DW_FORM_data1 |
            constants::DW_FORM_ref1 => Some(1),

            constants::DW_FORM_data2 |
            constants::DW_FORM_ref2 => Some(2),

            constants::DW_FORM_data4 |
            constants::DW_FORM_ref4 => Some(4),

            constants::DW_FORM_data8 |
            constants::DW_FORM_ref8 => Some(8),

            constants::DW_FORM_sec_offset |
            constants::DW_FORM_ref_addr |
            constants::DW_FORM_ref_sig8 |
            constants::DW_FORM_strp => Some(header.format().word_size() as usize),

            // Variably sized forms.
            constants::DW_FORM_block |
            constants::DW_FORM_block1 |
            constants::DW_FORM_block2 |
            constants::DW_FORM_block4 |
            constants::DW_FORM_exprloc |
            constants::DW_FORM_ref_udata |
            constants::DW_FORM_string |
            constants::DW_FORM_sdata |
            constants::DW_FORM_udata |
            constants::DW_FORM_indirect |

            // We don't know the size of unknown forms.
            _ => None,
        }
    }

    /// Parse an attribute's form.
    fn parse_form<R: Reader>(input: &mut R) -> Result<constants::DwForm> {
        let val = input.read_uleb128()?;
        if val == 0 {
            Err(Error::AttributeFormZero)
        } else {
            Ok(constants::DwForm(val))
        }
    }

    /// Parse an attribute specification. Returns `None` for the null attribute
    /// specification, `Some` for an actual attribute specification.
    fn parse<R: Reader>(input: &mut R) -> Result<Option<AttributeSpecification>> {
        let name = input.read_uleb128()?;
        if name == 0 {
            // Parse the null attribute specification.
            let form = input.read_uleb128()?;
            return if form == 0 {
                Ok(None)
            } else {
                Err(Error::ExpectedZero)
            };
        }

        let name = constants::DwAt(name);
        let form = Self::parse_form(input)?;
        let implicit_const_value = if form == constants::DW_FORM_implicit_const {
            Some(input.read_sleb128()?)
        } else {
            None
        };
        let spec = AttributeSpecification::new(name, form, implicit_const_value);
        Ok(Some(spec))
    }
}

#[cfg(test)]
pub mod tests {
    use test_assembler::Section;
    use super::*;
    use crate::constants;
    use crate::endianity::LittleEndian;
    use crate::read::{EndianSlice, Error};
    #[cfg(target_pointer_width = "32")]
    use std::u32;
    use crate::test_util::GimliSectionMethods;

    pub trait AbbrevSectionMethods {
        fn abbrev(self, code: u64, tag: constants::DwTag, children: constants::DwChildren) -> Self;
        fn abbrev_null(self) -> Self;
        fn abbrev_attr(self, name: constants::DwAt, form: constants::DwForm) -> Self;
        fn abbrev_attr_implicit_const(self, name: constants::DwAt, value: i64) -> Self;
        fn abbrev_attr_null(self) -> Self;
    }

    impl AbbrevSectionMethods for Section {
        fn abbrev(self, code: u64, tag: constants::DwTag, children: constants::DwChildren) -> Self {
            self.uleb(code).uleb(tag.0).D8(children.0)
        }

        fn abbrev_null(self) -> Self {
            self.D8(0)
        }

        fn abbrev_attr(self, name: constants::DwAt, form: constants::DwForm) -> Self {
            self.uleb(name.0).uleb(form.0)
        }

        fn abbrev_attr_implicit_const(self, name: constants::DwAt, value: i64) -> Self {
            self.uleb(name.0)
                .uleb(constants::DW_FORM_implicit_const.0)
                .sleb(value)
        }

        fn abbrev_attr_null(self) -> Self {
            self.D8(0).D8(0)
        }
    }

    #[test]
    fn test_debug_abbrev_ok() {
        let extra_start = [1, 2, 3, 4];
        let expected_rest = [5, 6, 7, 8];
        #[cfg_attr(rustfmt, rustfmt_skip)]
        let buf = Section::new()
            .append_bytes(&extra_start)
            .abbrev(2, constants::DW_TAG_subprogram, constants::DW_CHILDREN_no)
                .abbrev_attr(constants::DW_AT_name, constants::DW_FORM_string)
                .abbrev_attr_null()
            .abbrev(1, constants::DW_TAG_compile_unit, constants::DW_CHILDREN_yes)
                .abbrev_attr(constants::DW_AT_producer, constants::DW_FORM_strp)
                .abbrev_attr(constants::DW_AT_language, constants::DW_FORM_data2)
                .abbrev_attr_null()
            .abbrev_null()
            .append_bytes(&expected_rest)
            .get_contents()
            .unwrap();

        let abbrev1 = Abbreviation::new(
            1,
            constants::DW_TAG_compile_unit,
            constants::DW_CHILDREN_yes,
            vec![
                AttributeSpecification::new(
                    constants::DW_AT_producer,
                    constants::DW_FORM_strp,
                    None,
                ),
                AttributeSpecification::new(
                    constants::DW_AT_language,
                    constants::DW_FORM_data2,
                    None,
                ),
            ],
        );

        let abbrev2 = Abbreviation::new(
            2,
            constants::DW_TAG_subprogram,
            constants::DW_CHILDREN_no,
            vec![AttributeSpecification::new(
                constants::DW_AT_name,
                constants::DW_FORM_string,
                None,
            )],
        );

        let debug_abbrev = DebugAbbrev::new(&buf, LittleEndian);
        let debug_abbrev_offset = DebugAbbrevOffset(extra_start.len());
        let abbrevs = debug_abbrev
            .abbreviations(debug_abbrev_offset)
            .expect("Should parse abbreviations");
        assert_eq!(abbrevs.get(1), Some(&abbrev1));
        assert_eq!(abbrevs.get(2), Some(&abbrev2));
    }

    #[test]
    fn test_abbreviations_insert() {
        fn abbrev(code: u64) -> Abbreviation {
            Abbreviation::new(
                code,
                constants::DwTag(code),
                constants::DW_CHILDREN_no,
                vec![],
            )
        }

        fn assert_abbrev(abbrevs: &Abbreviations, code: u64) {
            let abbrev = abbrevs.get(code).unwrap();
            assert_eq!(abbrev.tag(), constants::DwTag(code));
        }

        // Sequential insert.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(1)).unwrap();
        abbrevs.insert(abbrev(2)).unwrap();
        assert_eq!(abbrevs.vec.len(), 2);
        assert!(abbrevs.map.is_empty());
        assert_abbrev(&abbrevs, 1);
        assert_abbrev(&abbrevs, 2);

        // Out of order insert.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(2)).unwrap();
        abbrevs.insert(abbrev(3)).unwrap();
        assert!(abbrevs.vec.is_empty());
        assert_abbrev(&abbrevs, 2);
        assert_abbrev(&abbrevs, 3);

        // Mixed order insert.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(1)).unwrap();
        abbrevs.insert(abbrev(3)).unwrap();
        abbrevs.insert(abbrev(2)).unwrap();
        assert_eq!(abbrevs.vec.len(), 2);
        assert_abbrev(&abbrevs, 1);
        assert_abbrev(&abbrevs, 2);
        assert_abbrev(&abbrevs, 3);

        // Duplicate code in vec.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(1)).unwrap();
        abbrevs.insert(abbrev(2)).unwrap();
        assert_eq!(abbrevs.insert(abbrev(1)), Err(()));
        assert_eq!(abbrevs.insert(abbrev(2)), Err(()));

        // Duplicate code in map when adding to map.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(2)).unwrap();
        assert_eq!(abbrevs.insert(abbrev(2)), Err(()));

        // Duplicate code in map when adding to vec.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(2)).unwrap();
        abbrevs.insert(abbrev(1)).unwrap();
        assert_eq!(abbrevs.insert(abbrev(2)), Err(()));

        // 32-bit usize conversions.
        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(2)).unwrap();
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn test_abbreviations_insert_32() {
        fn abbrev(code: u64) -> Abbreviation {
            Abbreviation::new(
                code,
                constants::DwTag(code),
                constants::DW_CHILDREN_no,
                vec![],
            )
        }

        fn assert_abbrev(abbrevs: &Abbreviations, code: u64) {
            let abbrev = abbrevs.get(code).unwrap();
            assert_eq!(abbrev.tag(), constants::DwTag(code));
        }

        let mut abbrevs = Abbreviations::empty();
        abbrevs.insert(abbrev(1)).unwrap();

        let wrap_code = (u32::MAX as u64 + 1) + 1;
        // `get` should not treat the wrapped code as `1`.
        assert_eq!(abbrevs.get(wrap_code), None);
        // `insert` should not treat the wrapped code as `1`.
        abbrevs.insert(abbrev(wrap_code)).unwrap();
        assert_abbrev(&abbrevs, 1);
        assert_abbrev(&abbrevs, wrap_code);
    }

    #[test]
    fn test_parse_abbreviations_ok() {
        let expected_rest = [1, 2, 3, 4];
        #[cfg_attr(rustfmt, rustfmt_skip)]
        let buf = Section::new()
            .abbrev(2, constants::DW_TAG_subprogram, constants::DW_CHILDREN_no)
                .abbrev_attr(constants::DW_AT_name, constants::DW_FORM_string)
                .abbrev_attr_null()
            .abbrev(1, constants::DW_TAG_compile_unit, constants::DW_CHILDREN_yes)
                .abbrev_attr(constants::DW_AT_producer, constants::DW_FORM_strp)
                .abbrev_attr(constants::DW_AT_language, constants::DW_FORM_data2)
                .abbrev_attr_null()
            .abbrev_null()
            .append_bytes(&expected_rest)
            .get_contents()
            .unwrap();
        let rest = &mut EndianSlice::new(&*buf, LittleEndian);

        let abbrev1 = Abbreviation::new(
            1,
            constants::DW_TAG_compile_unit,
            constants::DW_CHILDREN_yes,
            vec![
                AttributeSpecification::new(
                    constants::DW_AT_producer,
                    constants::DW_FORM_strp,
                    None,
                ),
                AttributeSpecification::new(
                    constants::DW_AT_language,
                    constants::DW_FORM_data2,
                    None,
                ),
            ],
        );

        let abbrev2 = Abbreviation::new(
            2,
            constants::DW_TAG_subprogram,
            constants::DW_CHILDREN_no,
            vec![AttributeSpecification::new(
                constants::DW_AT_name,
                constants::DW_FORM_string,
                None,
            )],
        );

        let abbrevs = Abbreviations::parse(rest).expect("Should parse abbreviations");
        assert_eq!(abbrevs.get(1), Some(&abbrev1));
        assert_eq!(abbrevs.get(2), Some(&abbrev2));
        assert_eq!(*rest, EndianSlice::new(&expected_rest, LittleEndian));
    }

    #[test]
    fn test_parse_abbreviations_duplicate() {
        let expected_rest = [1, 2, 3, 4];
        #[cfg_attr(rustfmt, rustfmt_skip)]
        let buf = Section::new()
            .abbrev(1, constants::DW_TAG_subprogram, constants::DW_CHILDREN_no)
                .abbrev_attr(constants::DW_AT_name, constants::DW_FORM_string)
                .abbrev_attr_null()
            .abbrev(1, constants::DW_TAG_compile_unit, constants::DW_CHILDREN_yes)
                .abbrev_attr(constants::DW_AT_producer, constants::DW_FORM_strp)
                .abbrev_attr(constants::DW_AT_language, constants::DW_FORM_data2)
                .abbrev_attr_null()
            .abbrev_null()
            .append_bytes(&expected_rest)
            .get_contents()
            .unwrap();
        let buf = &mut EndianSlice::new(&*buf, LittleEndian);

        match Abbreviations::parse(buf) {
            Err(Error::DuplicateAbbreviationCode) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        };
    }

    #[test]
    fn test_parse_abbreviation_tag_ok() {
        let buf = [0x01, 0x02];
        let rest = &mut EndianSlice::new(&buf, LittleEndian);
        let tag = Abbreviation::parse_tag(rest).expect("Should parse tag");
        assert_eq!(tag, constants::DW_TAG_array_type);
        assert_eq!(*rest, EndianSlice::new(&buf[1..], LittleEndian));
    }

    #[test]
    fn test_parse_abbreviation_tag_zero() {
        let buf = [0x00];
        let buf = &mut EndianSlice::new(&buf, LittleEndian);
        match Abbreviation::parse_tag(buf) {
            Err(Error::AbbreviationTagZero) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        };
    }

    #[test]
    fn test_parse_abbreviation_has_children() {
        let buf = [0x00, 0x01, 0x02];
        let rest = &mut EndianSlice::new(&buf, LittleEndian);
        let val = Abbreviation::parse_has_children(rest).expect("Should parse children");
        assert_eq!(val, constants::DW_CHILDREN_no);
        let val = Abbreviation::parse_has_children(rest).expect("Should parse children");
        assert_eq!(val, constants::DW_CHILDREN_yes);
        match Abbreviation::parse_has_children(rest) {
            Err(Error::BadHasChildren) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        };
    }

    #[test]
    fn test_parse_abbreviation_ok() {
        let expected_rest = [0x01, 0x02, 0x03, 0x04];
        let buf = Section::new()
            .abbrev(1, constants::DW_TAG_subprogram, constants::DW_CHILDREN_no)
            .abbrev_attr(constants::DW_AT_name, constants::DW_FORM_string)
            .abbrev_attr_null()
            .append_bytes(&expected_rest)
            .get_contents()
            .unwrap();
        let rest = &mut EndianSlice::new(&*buf, LittleEndian);

        let expect = Some(Abbreviation::new(
            1,
            constants::DW_TAG_subprogram,
            constants::DW_CHILDREN_no,
            vec![AttributeSpecification::new(
                constants::DW_AT_name,
                constants::DW_FORM_string,
                None,
            )],
        ));

        let abbrev = Abbreviation::parse(rest).expect("Should parse abbreviation");
        assert_eq!(abbrev, expect);
        assert_eq!(*rest, EndianSlice::new(&expected_rest, LittleEndian));
    }

    #[test]
    fn test_parse_abbreviation_implicit_const_ok() {
        let expected_rest = [0x01, 0x02, 0x03, 0x04];
        let buf = Section::new()
            .abbrev(1, constants::DW_TAG_subprogram, constants::DW_CHILDREN_no)
            .abbrev_attr_implicit_const(constants::DW_AT_name, -42)
            .abbrev_attr_null()
            .append_bytes(&expected_rest)
            .get_contents()
            .unwrap();
        let rest = &mut EndianSlice::new(&*buf, LittleEndian);

        let expect = Some(Abbreviation::new(
            1,
            constants::DW_TAG_subprogram,
            constants::DW_CHILDREN_no,
            vec![AttributeSpecification::new(
                constants::DW_AT_name,
                constants::DW_FORM_implicit_const,
                Some(-42),
            )],
        ));

        let abbrev = Abbreviation::parse(rest).expect("Should parse abbreviation");
        assert_eq!(abbrev, expect);
        assert_eq!(*rest, EndianSlice::new(&expected_rest, LittleEndian));
    }

    #[test]
    fn test_parse_abbreviation_implicit_const_no_const() {
        let buf = Section::new()
            .abbrev(1, constants::DW_TAG_subprogram, constants::DW_CHILDREN_no)
            .abbrev_attr(constants::DW_AT_name, constants::DW_FORM_implicit_const)
            .get_contents()
            .unwrap();
        let buf = &mut EndianSlice::new(&*buf, LittleEndian);

        match Abbreviation::parse(buf) {
            Err(Error::UnexpectedEof) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        }
    }

    #[test]
    fn test_parse_null_abbreviation_ok() {
        let expected_rest = [0x01, 0x02, 0x03, 0x04];
        let buf = Section::new()
            .abbrev_null()
            .append_bytes(&expected_rest)
            .get_contents()
            .unwrap();
        let rest = &mut EndianSlice::new(&*buf, LittleEndian);

        let abbrev = Abbreviation::parse(rest).expect("Should parse null abbreviation");
        assert!(abbrev.is_none());
        assert_eq!(*rest, EndianSlice::new(&expected_rest, LittleEndian));
    }

    #[test]
    fn test_parse_attribute_form_ok() {
        let buf = [0x01, 0x02];
        let rest = &mut EndianSlice::new(&buf, LittleEndian);
        let tag = AttributeSpecification::parse_form(rest).expect("Should parse form");
        assert_eq!(tag, constants::DW_FORM_addr);
        assert_eq!(*rest, EndianSlice::new(&buf[1..], LittleEndian));
    }

    #[test]
    fn test_parse_attribute_form_zero() {
        let buf = [0x00];
        let buf = &mut EndianSlice::new(&buf, LittleEndian);
        match AttributeSpecification::parse_form(buf) {
            Err(Error::AttributeFormZero) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        };
    }

    #[test]
    fn test_parse_null_attribute_specification_ok() {
        let buf = [0x00, 0x00, 0x01];
        let rest = &mut EndianSlice::new(&buf, LittleEndian);
        let attr =
            AttributeSpecification::parse(rest).expect("Should parse null attribute specification");
        assert!(attr.is_none());
        assert_eq!(*rest, EndianSlice::new(&buf[2..], LittleEndian));
    }

    #[test]
    fn test_parse_attribute_specifications_name_zero() {
        let buf = [0x00, 0x01, 0x00, 0x00];
        let buf = &mut EndianSlice::new(&buf, LittleEndian);
        match AttributeSpecification::parse(buf) {
            Err(Error::ExpectedZero) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        };
    }

    #[test]
    fn test_parse_attribute_specifications_form_zero() {
        let buf = [0x01, 0x00, 0x00, 0x00];
        let buf = &mut EndianSlice::new(&buf, LittleEndian);
        match AttributeSpecification::parse(buf) {
            Err(Error::AttributeFormZero) => {}
            otherwise => panic!("Unexpected result: {:?}", otherwise),
        };
    }
}
