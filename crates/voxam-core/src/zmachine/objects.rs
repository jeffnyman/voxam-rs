//! The object table: attributes, family relations, properties (§12).
//!
//! Objects live in a table in dynamic memory whose address the
//! header word at $0a gives (§12.1). Each has attribute flags, a
//! parent, a sibling, and a child, plus its own property table. The
//! geometry forks at Version 4: entry sizes, attribute counts,
//! object limits, and the property size-byte format all change.
//!
//! One departure from the Python reference, forced by borrowing:
//! there the table holds its memory image; here `ObjectTable` is
//! the fixed geometry alone, and every method takes the memory it
//! reads or writes -- so the machine that owns both stays free to
//! use each as it needs.

use crate::errors::VoxamError;
use crate::zmachine::memory::Memory;

const V3_LAST_VERSION: u8 = 3;

/// The property defaults table opens the object table: 31 words
/// through Version 3, 63 after (§12.2).
const V3_DEFAULTS: usize = 31;
const V4_DEFAULTS: usize = 63;

/// Entries are 9 bytes with 32 attribute flags and byte-sized
/// family relations through Version 3; 14 bytes, 48 flags, and
/// word-sized relations after (§12.3.1, §12.3.2).
const V3_ENTRY_SIZE: usize = 9;
const V4_ENTRY_SIZE: usize = 14;
const V3_ATTRIBUTE_BYTES: usize = 4;
const V4_ATTRIBUTE_BYTES: usize = 6;
const V3_MAX_OBJECT: u16 = 255;
const V4_MAX_OBJECT: u16 = 65535;

/// Version 1 to 3 property size bytes: 32 * (length - 1) + number
/// (§12.4.1). A zero size byte terminates the list.
const V3_PROPERTY_NUMBER_MASK: u8 = 0x1F;
const V3_SIZE_FACTOR: u8 = 32;
const TERMINATOR: u8 = 0;

/// Version 4 and later size bytes: the number is the bottom 6 bits;
/// bit 7 set means a second byte carries the length in its bottom 6
/// bits, with 0 meaning 64; bit 7 clear means bit 6 selects a
/// length of 2 over 1 (§12.4.2).
const V4_PROPERTY_NUMBER_MASK: u8 = 0x3F;
const TWO_BYTE_SIZE_BIT: u8 = 0x80;
const ONE_BYTE_LENGTH_BIT: u8 = 0x40;
const V4_LENGTH_MASK: u8 = 0x3F;
const ZERO_MEANS_64: usize = 64;

/// get_prop and put_prop only handle lengths 1 and 2 (§15).
const WORD_LENGTH: usize = 2;

const WORD_SIZE: usize = 2;
const RELATION_COUNT: usize = 3;

/// The three family relations, in their entry order (§12.3).
#[derive(Clone, Copy)]
enum Relation {
    Parent = 0,
    Sibling = 1,
    Child = 2,
}

/// The version-fixed geometry of the object table (§12).
pub struct ObjectTable {
    base: usize,
    attribute_count: u16,
    attribute_bytes: usize,
    entry_size: usize,
    defaults: usize,
    max_object: u16,
    v3: bool,
    entries: usize,
}

impl ObjectTable {
    /// Fix the version-dependent geometry over an image.
    pub fn new(memory: &Memory) -> Self {
        let base = usize::from(memory.header().object_table_address());
        let v3 = memory.header().version() <= V3_LAST_VERSION;
        let attribute_bytes = if v3 {
            V3_ATTRIBUTE_BYTES
        } else {
            V4_ATTRIBUTE_BYTES
        };
        let defaults = if v3 { V3_DEFAULTS } else { V4_DEFAULTS };

        Self {
            base,
            attribute_count: 8 * attribute_bytes as u16,
            attribute_bytes,
            entry_size: if v3 { V3_ENTRY_SIZE } else { V4_ENTRY_SIZE },
            defaults,
            max_object: if v3 { V3_MAX_OBJECT } else { V4_MAX_OBJECT },
            v3,
            entries: base + WORD_SIZE * defaults,
        }
    }

    /// Read a property's default value (§12.2), refusing numbers
    /// past the table.
    pub fn default(&self, memory: &Memory, number: u16) -> Result<u16, VoxamError> {
        if number < 1 || usize::from(number) > self.defaults {
            return Err(object_error(format!(
                "property {number} has no default; the table holds {} (§12.2)",
                self.defaults
            )));
        }

        memory.read_word(self.base + WORD_SIZE * usize::from(number - 1))
    }

    /// Test an attribute flag (§12.3.1).
    pub fn attribute(&self, memory: &Memory, obj: u16, attribute: u16) -> Result<bool, VoxamError> {
        let (address, bit) = self.attribute_location(obj, attribute)?;

        Ok(memory.read_byte(address)? & bit != 0)
    }

    /// Set or clear an attribute flag (§12.3.1).
    pub fn set_attribute(
        &self,
        memory: &mut Memory,
        obj: u16,
        attribute: u16,
        on: bool,
    ) -> Result<(), VoxamError> {
        let (address, bit) = self.attribute_location(obj, attribute)?;
        let flags = memory.read_byte(address)?;
        let flags = if on { flags | bit } else { flags & !bit };

        memory.write_byte(address, flags)
    }

    /// The object's parent, 0 for none (§12.3).
    pub fn parent(&self, memory: &Memory, obj: u16) -> Result<u16, VoxamError> {
        self.relation(memory, obj, Relation::Parent)
    }

    /// The object's next sibling, 0 for none (§12.3).
    pub fn sibling(&self, memory: &Memory, obj: u16) -> Result<u16, VoxamError> {
        self.relation(memory, obj, Relation::Sibling)
    }

    /// The object's first child, 0 for none (§12.3).
    pub fn child(&self, memory: &Memory, obj: u16) -> Result<u16, VoxamError> {
        self.relation(memory, obj, Relation::Child)
    }

    /// Detach an object from its parent (§15 remove_obj).
    ///
    /// Its children stay with it; it keeps no stale sibling link.
    pub fn remove(&self, memory: &mut Memory, obj: u16) -> Result<(), VoxamError> {
        let parent = self.parent(memory, obj)?;

        if parent == 0 {
            return Ok(());
        }

        let following = self.sibling(memory, obj)?;

        if self.child(memory, parent)? == obj {
            self.set_relation(memory, parent, Relation::Child, following)?;
        } else {
            let mut previous = self.child(memory, parent)?;

            while self.sibling(memory, previous)? != obj {
                previous = self.sibling(memory, previous)?;
            }

            self.set_relation(memory, previous, Relation::Sibling, following)?;
        }

        self.set_relation(memory, obj, Relation::Parent, 0)?;
        self.set_relation(memory, obj, Relation::Sibling, 0)
    }

    /// Move an object to be a destination's first child (§15
    /// insert_obj).
    pub fn insert(
        &self,
        memory: &mut Memory,
        obj: u16,
        destination: u16,
    ) -> Result<(), VoxamError> {
        self.remove(memory, obj)?;

        let first = self.child(memory, destination)?;
        self.set_relation(memory, obj, Relation::Sibling, first)?;
        self.set_relation(memory, obj, Relation::Parent, destination)?;
        self.set_relation(memory, destination, Relation::Child, obj)
    }

    /// The byte address of the object's encoded short name (§12.4).
    pub fn short_name_address(&self, memory: &Memory, obj: u16) -> Result<usize, VoxamError> {
        Ok(self.properties_address(memory, obj)? + 1)
    }

    /// Find a property the object itself provides (§12.4),
    /// returning the property data's address and length, or `None`
    /// when the object does not provide the property.
    pub fn find_property(
        &self,
        memory: &Memory,
        obj: u16,
        number: u16,
    ) -> Result<Option<(usize, usize)>, VoxamError> {
        let mut address = self.first_property(memory, obj)?;

        loop {
            let Some((found_number, length, data)) = self.property_at(memory, address)? else {
                return Ok(None);
            };

            if found_number == number {
                return Ok(Some((data, length)));
            }

            address = data + length;
        }
    }

    /// Read a property, falling back to its default (§15 get_prop).
    ///
    /// Refuses a property longer than a word, which get_prop may
    /// not read (§15).
    pub fn property_value(
        &self,
        memory: &Memory,
        obj: u16,
        number: u16,
    ) -> Result<u16, VoxamError> {
        let Some((data, length)) = self.find_property(memory, obj, number)? else {
            return self.default(memory, number);
        };

        if length == 1 {
            return Ok(u16::from(memory.read_byte(data)?));
        }

        if length == WORD_LENGTH {
            return memory.read_word(data);
        }

        Err(object_error(format!(
            "get_prop may not read property {number} of object {obj}: its length is \
             {length}, not 1 or 2 (§15)"
        )))
    }

    /// Write a property the object must provide (§15 put_prop).
    ///
    /// A length-1 property takes the least significant byte.
    /// Refuses an absent property, or one longer than a word.
    pub fn put_property(
        &self,
        memory: &mut Memory,
        obj: u16,
        number: u16,
        value: u16,
    ) -> Result<(), VoxamError> {
        let Some((data, length)) = self.find_property(memory, obj, number)? else {
            return Err(object_error(format!(
                "object {obj} does not provide property {number}, so put_prop must \
                 halt (§15)"
            )));
        };

        if length == 1 {
            return memory.write_byte(data, (value & 0xFF) as u8);
        }

        if length == WORD_LENGTH {
            return memory.write_word(data, value);
        }

        Err(object_error(format!(
            "put_prop may not write property {number} of object {obj}: its length is \
             {length}, not 1 or 2 (§15)"
        )))
    }

    /// Recover a property's length from its data address (§12.4).
    ///
    /// The size information sits just before the data: a lone size
    /// byte through Version 3; afterward, a byte whose top bit
    /// tells whether it is the second of two (carrying a length) or
    /// alone (bit 6 selecting 2 over 1).
    pub fn property_length_at(
        &self,
        memory: &Memory,
        data_address: usize,
    ) -> Result<usize, VoxamError> {
        let size_byte = memory.read_byte(data_address - 1)?;

        if self.v3 {
            return Ok(usize::from(size_byte / V3_SIZE_FACTOR) + 1);
        }

        if size_byte & TWO_BYTE_SIZE_BIT != 0 {
            let length = usize::from(size_byte & V4_LENGTH_MASK);

            return Ok(if length == 0 { ZERO_MEANS_64 } else { length });
        }

        Ok(if size_byte & ONE_BYTE_LENGTH_BIT != 0 {
            2
        } else {
            1
        })
    }

    /// Walk the property list (§15 get_next_prop).
    ///
    /// Number 0 asks for the first property; otherwise the property
    /// after the given one, which must be present. The result 0
    /// means the list ended.
    pub fn next_property(&self, memory: &Memory, obj: u16, number: u16) -> Result<u16, VoxamError> {
        if number == 0 {
            let first = self.first_property(memory, obj)?;

            return Ok(match self.property_at(memory, first)? {
                Some((found_number, _, _)) => found_number,
                None => 0,
            });
        }

        let Some((data, length)) = self.find_property(memory, obj, number)? else {
            return Err(object_error(format!(
                "object {obj} does not provide property {number}, so get_next_prop \
                 must halt (§15)"
            )));
        };

        Ok(match self.property_at(memory, data + length)? {
            Some((following, _, _)) => following,
            None => 0,
        })
    }

    /// Whether a number names an attribute in this version
    /// (§12.3.1).
    pub fn attribute_exists(&self, attribute: u16) -> bool {
        attribute < self.attribute_count
    }

    /// Locate an object's entry, policing the number (§12.3).
    fn entry(&self, obj: u16) -> Result<usize, VoxamError> {
        if obj < 1 || obj > self.max_object {
            return Err(object_error(format!(
                "object {obj} does not exist: object numbers run from 1 to {}, with 0 \
                 meaning nothing (§12.3)",
                self.max_object
            )));
        }

        Ok(self.entries + usize::from(obj - 1) * self.entry_size)
    }

    /// Locate the byte and bit of an attribute flag (§12.3.1).
    fn attribute_location(&self, obj: u16, attribute: u16) -> Result<(usize, u8), VoxamError> {
        if attribute >= self.attribute_count {
            return Err(object_error(format!(
                "attribute {attribute} does not exist: attributes run from 0 to {} \
                 (§12.3)",
                self.attribute_count - 1
            )));
        }

        let address = self.entry(obj)? + usize::from(attribute / 8);
        let bit = 0x80u8 >> (attribute % 8);

        Ok((address, bit))
    }

    /// Read parent, sibling, or child (§12.3).
    fn relation(&self, memory: &Memory, obj: u16, relation: Relation) -> Result<u16, VoxamError> {
        let base = self.entry(obj)? + self.attribute_bytes;
        let index = relation as usize;

        if self.v3 {
            return Ok(u16::from(memory.read_byte(base + index)?));
        }

        memory.read_word(base + WORD_SIZE * index)
    }

    /// Write parent, sibling, or child (§12.3).
    fn set_relation(
        &self,
        memory: &mut Memory,
        obj: u16,
        relation: Relation,
        value: u16,
    ) -> Result<(), VoxamError> {
        let base = self.entry(obj)? + self.attribute_bytes;
        let index = relation as usize;

        if self.v3 {
            return memory.write_byte(base + index, (value & 0xFF) as u8);
        }

        memory.write_word(base + WORD_SIZE * index, value)
    }

    /// The byte address of the object's property table (§12.3).
    fn properties_address(&self, memory: &Memory, obj: u16) -> Result<usize, VoxamError> {
        let relation_bytes = RELATION_COUNT * if self.v3 { 1 } else { WORD_SIZE };

        Ok(usize::from(memory.read_word(
            self.entry(obj)? + self.attribute_bytes + relation_bytes,
        )?))
    }

    /// The address of the first property block, past the name
    /// (§12.4).
    fn first_property(&self, memory: &Memory, obj: u16) -> Result<usize, VoxamError> {
        let table = self.properties_address(memory, obj)?;
        let name_words = usize::from(memory.read_byte(table)?);

        Ok(table + 1 + WORD_SIZE * name_words)
    }

    /// Read a property block header (§12.4.1, §12.4.2), returning
    /// the property number, data length, and data address, or
    /// `None` at the list terminator.
    fn property_at(
        &self,
        memory: &Memory,
        address: usize,
    ) -> Result<Option<(u16, usize, usize)>, VoxamError> {
        let first = memory.read_byte(address)?;

        if first == TERMINATOR {
            return Ok(None);
        }

        if self.v3 {
            let number = u16::from(first & V3_PROPERTY_NUMBER_MASK);
            let length = usize::from(first / V3_SIZE_FACTOR) + 1;

            return Ok(Some((number, length, address + 1)));
        }

        let number = u16::from(first & V4_PROPERTY_NUMBER_MASK);

        if first & TWO_BYTE_SIZE_BIT != 0 {
            let length = usize::from(memory.read_byte(address + 1)? & V4_LENGTH_MASK);
            let length = if length == 0 { ZERO_MEANS_64 } else { length };

            return Ok(Some((number, length, address + 2)));
        }

        let length = if first & ONE_BYTE_LENGTH_BIT != 0 {
            2
        } else {
            1
        };

        Ok(Some((number, length, address + 1)))
    }
}

fn object_error(message: String) -> VoxamError {
    VoxamError::ZMachineObject(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::story::Story;

    const TABLE_BASE: usize = 0x400;

    /// 'h' and 'i' in one terminated word (§3.5.3).
    const HI: [u8; 2] = [0xB5, 0xC5];

    #[derive(Default, Clone)]
    struct Obj {
        attributes: Vec<u16>,
        parent: u16,
        sibling: u16,
        child: u16,
        name: Vec<u8>,
        /// In descending property-number order, as §12.4 stores them.
        properties: Vec<(u8, Vec<u8>)>,
    }

    /// The reference suite's table builder, byte for byte.
    fn build_table(objects: &[Obj], version: u8, defaults: &[(usize, u16)]) -> Vec<u8> {
        let v3 = version <= V3_LAST_VERSION;
        let default_count = if v3 { 31 } else { 63 };
        let entry_size = if v3 { 9 } else { 14 };
        let attribute_bytes = if v3 { 4 } else { 6 };

        let mut data = Vec::new();

        for number in 1..=default_count {
            let value = defaults
                .iter()
                .find(|(n, _)| *n == number)
                .map_or(0, |(_, v)| *v);
            data.extend_from_slice(&value.to_be_bytes());
        }

        let entries_start = data.len();
        data.resize(data.len() + entry_size * objects.len(), 0);

        let mut property_tables = Vec::new();

        for obj in objects {
            property_tables.push(TABLE_BASE + data.len());
            data.push((obj.name.len() / 2) as u8);
            data.extend_from_slice(&obj.name);

            for (number, payload) in &obj.properties {
                if v3 {
                    data.push(32 * (payload.len() as u8 - 1) + number);
                } else if payload.len() == 1 {
                    data.push(*number);
                } else if payload.len() == WORD_LENGTH {
                    data.push(0x40 | number);
                } else {
                    data.push(0x80 | number);
                    data.push(0x80 | (payload.len() as u8 & 0x3F));
                }

                data.extend_from_slice(payload);
            }

            data.push(0);
        }

        for (index, obj) in objects.iter().enumerate() {
            let offset = entries_start + index * entry_size;
            let mut entry = vec![0u8; attribute_bytes];

            for attribute in &obj.attributes {
                entry[usize::from(attribute / 8)] |= 0x80 >> (attribute % 8);
            }

            if v3 {
                entry.extend_from_slice(&[obj.parent as u8, obj.sibling as u8, obj.child as u8]);
            } else {
                for relation in [obj.parent, obj.sibling, obj.child] {
                    entry.extend_from_slice(&relation.to_be_bytes());
                }
            }

            entry.extend_from_slice(&(property_tables[index] as u16).to_be_bytes());
            data[offset..offset + entry_size].copy_from_slice(&entry);
        }

        data
    }

    fn scene_memory_of(objects: &[Obj], version: u8, defaults: &[(usize, u16)]) -> Memory {
        let mut data = vec![0u8; 2048];
        data[0] = version;
        data[0x04..0x06].copy_from_slice(&0x0700u16.to_be_bytes());
        data[0x0A..0x0C].copy_from_slice(&(TABLE_BASE as u16).to_be_bytes());
        data[0x0C..0x0E].copy_from_slice(&0x0100u16.to_be_bytes());
        data[0x0E..0x10].copy_from_slice(&0x0700u16.to_be_bytes());

        let table = build_table(objects, version, defaults);
        data[TABLE_BASE..TABLE_BASE + table.len()].copy_from_slice(&table);

        Memory::new(&Story::new(data).unwrap()).unwrap()
    }

    /// The standing scene: a box holding a coin then a key. The
    /// box's properties are 5 (a word) and 3 (a byte); property 7
    /// exists only as a table default.
    fn scene(version: u8) -> Memory {
        scene_memory_of(
            &[
                Obj {
                    attributes: vec![3, 12],
                    child: 2,
                    name: HI.to_vec(),
                    properties: vec![(5, vec![0x12, 0x34]), (3, vec![0x42])],
                    ..Obj::default()
                },
                Obj {
                    parent: 1,
                    sibling: 3,
                    ..Obj::default()
                },
                Obj {
                    parent: 1,
                    ..Obj::default()
                },
            ],
            version,
            &[(7, 0x0777)],
        )
    }

    #[test]
    fn reads_and_writes_attributes() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        assert!(table.attribute(&memory, 1, 3).unwrap());
        assert!(table.attribute(&memory, 1, 12).unwrap());
        assert!(!table.attribute(&memory, 1, 0).unwrap());

        table.set_attribute(&mut memory, 1, 0, true).unwrap();
        assert!(table.attribute(&memory, 1, 0).unwrap());

        table.set_attribute(&mut memory, 1, 3, false).unwrap();
        assert!(!table.attribute(&memory, 1, 3).unwrap());
    }

    #[test]
    fn attribute_ranges_fork_at_version_4() {
        let v3 = scene(3);
        let table = ObjectTable::new(&v3);

        assert!(table.attribute_exists(31));
        assert!(!table.attribute_exists(32));
        assert!(table.attribute(&v3, 1, 32).is_err());

        let v4 = scene(4);
        let table = ObjectTable::new(&v4);

        assert!(table.attribute_exists(47));
        assert!(!table.attribute_exists(48));
        assert!(!table.attribute(&v4, 1, 47).unwrap());
    }

    #[test]
    fn object_numbers_are_policed() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        let error = table.parent(&memory, 0).unwrap_err();
        assert!(error.to_string().contains("§12.3"));

        assert!(table.parent(&memory, 256).is_err());
    }

    #[test]
    fn reads_the_family_relations() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        assert_eq!(table.parent(&memory, 2).unwrap(), 1);
        assert_eq!(table.sibling(&memory, 2).unwrap(), 3);
        assert_eq!(table.child(&memory, 1).unwrap(), 2);
        assert_eq!(table.parent(&memory, 1).unwrap(), 0);
    }

    #[test]
    fn version_4_relations_are_words() {
        let memory = scene(4);
        let table = ObjectTable::new(&memory);

        assert_eq!(table.parent(&memory, 2).unwrap(), 1);
        assert_eq!(table.sibling(&memory, 2).unwrap(), 3);
        assert_eq!(table.child(&memory, 1).unwrap(), 2);
    }

    #[test]
    fn removing_the_first_child_promotes_its_sibling() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        table.remove(&mut memory, 2).unwrap();

        assert_eq!(table.child(&memory, 1).unwrap(), 3);
        assert_eq!(table.parent(&memory, 2).unwrap(), 0);
        assert_eq!(table.sibling(&memory, 2).unwrap(), 0);
    }

    #[test]
    fn removing_a_later_child_relinks_the_chain() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        table.remove(&mut memory, 3).unwrap();

        assert_eq!(table.child(&memory, 1).unwrap(), 2);
        assert_eq!(table.sibling(&memory, 2).unwrap(), 0);
        assert_eq!(table.parent(&memory, 3).unwrap(), 0);
    }

    #[test]
    fn version_4_tree_surgery_writes_words() {
        let mut memory = scene(4);
        let table = ObjectTable::new(&memory);

        table.remove(&mut memory, 2).unwrap();

        assert_eq!(table.child(&memory, 1).unwrap(), 3);

        table.insert(&mut memory, 2, 1).unwrap();

        assert_eq!(table.child(&memory, 1).unwrap(), 2);
        assert_eq!(table.sibling(&memory, 2).unwrap(), 3);
    }

    #[test]
    fn removing_a_parentless_object_changes_nothing() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        table.remove(&mut memory, 1).unwrap();

        assert_eq!(table.child(&memory, 1).unwrap(), 2);
        assert_eq!(table.parent(&memory, 1).unwrap(), 0);
    }

    #[test]
    fn insertion_makes_the_first_child() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        table.insert(&mut memory, 3, 2).unwrap();

        assert_eq!(table.child(&memory, 2).unwrap(), 3);
        assert_eq!(table.parent(&memory, 3).unwrap(), 2);
        assert_eq!(table.sibling(&memory, 3).unwrap(), 0);
        assert_eq!(table.sibling(&memory, 2).unwrap(), 0);
    }

    #[test]
    fn reads_properties_and_defaults() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        assert_eq!(table.property_value(&memory, 1, 5).unwrap(), 0x1234);
        assert_eq!(table.property_value(&memory, 1, 3).unwrap(), 0x42);

        // Property 7 exists only as a table default.
        assert_eq!(table.property_value(&memory, 1, 7).unwrap(), 0x0777);
        assert_eq!(table.default(&memory, 7).unwrap(), 0x0777);
    }

    #[test]
    fn writes_properties() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        table.put_property(&mut memory, 1, 5, 0xBEEF).unwrap();
        assert_eq!(table.property_value(&memory, 1, 5).unwrap(), 0xBEEF);

        // A length-1 property takes the least significant byte.
        table.put_property(&mut memory, 1, 3, 0x1234).unwrap();
        assert_eq!(table.property_value(&memory, 1, 3).unwrap(), 0x34);
    }

    #[test]
    fn writing_an_absent_property_halts() {
        let mut memory = scene(3);
        let table = ObjectTable::new(&memory);

        let error = table.put_property(&mut memory, 2, 5, 1).unwrap_err();

        assert_eq!(
            error.to_string(),
            "object 2 does not provide property 5, so put_prop must halt (§15)"
        );
    }

    #[test]
    fn long_properties_refuse_word_access() {
        let mut memory = scene_memory_of(
            &[Obj {
                properties: vec![(4, vec![1, 2, 3])],
                ..Obj::default()
            }],
            3,
            &[],
        );
        let table = ObjectTable::new(&memory);

        assert!(table.property_value(&memory, 1, 4).is_err());
        assert!(table.put_property(&mut memory, 1, 4, 1).is_err());

        // find_property still reports it, address and length both.
        let (_, length) = table.find_property(&memory, 1, 4).unwrap().unwrap();
        assert_eq!(length, 3);
    }

    #[test]
    fn property_lengths_recover_from_data_addresses() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        let (data, _) = table.find_property(&memory, 1, 5).unwrap().unwrap();
        assert_eq!(table.property_length_at(&memory, data).unwrap(), 2);

        let (data, _) = table.find_property(&memory, 1, 3).unwrap().unwrap();
        assert_eq!(table.property_length_at(&memory, data).unwrap(), 1);
    }

    #[test]
    fn version_4_property_formats() {
        let memory = scene_memory_of(
            &[Obj {
                properties: vec![
                    (40, vec![7; 64]),
                    (20, vec![1, 2, 3]),
                    (10, vec![0xAB, 0xCD]),
                    (4, vec![0x42]),
                ],
                ..Obj::default()
            }],
            4,
            &[],
        );
        let table = ObjectTable::new(&memory);

        assert_eq!(table.property_value(&memory, 1, 4).unwrap(), 0x42);
        assert_eq!(table.property_value(&memory, 1, 10).unwrap(), 0xABCD);

        let (data, length) = table.find_property(&memory, 1, 20).unwrap().unwrap();
        assert_eq!(length, 3);
        assert_eq!(table.property_length_at(&memory, data).unwrap(), 3);

        // A two-byte size whose length field is 0 means 64 (§12.4.2.1.1).
        let (data, length) = table.find_property(&memory, 1, 40).unwrap().unwrap();
        assert_eq!(length, 64);
        assert_eq!(table.property_length_at(&memory, data).unwrap(), 64);
    }

    #[test]
    fn walks_the_property_list() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        assert_eq!(table.next_property(&memory, 1, 0).unwrap(), 5);
        assert_eq!(table.next_property(&memory, 1, 5).unwrap(), 3);
        assert_eq!(table.next_property(&memory, 1, 3).unwrap(), 0);

        // An object with no properties has nothing first.
        assert_eq!(table.next_property(&memory, 2, 0).unwrap(), 0);

        let error = table.next_property(&memory, 2, 5).unwrap_err();
        assert!(error.to_string().contains("get_next_prop"));
    }

    #[test]
    fn defaults_are_policed() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        assert!(table.default(&memory, 0).is_err());
        assert!(table.default(&memory, 32).is_err());

        let v4 = scene(4);
        let table = ObjectTable::new(&v4);

        assert_eq!(table.default(&v4, 32).unwrap(), 0);
        assert!(table.default(&v4, 64).is_err());
    }

    #[test]
    fn short_names_decode() {
        let memory = scene(3);
        let table = ObjectTable::new(&memory);

        let address = table.short_name_address(&memory, 1).unwrap();
        let (name, _) = crate::zmachine::zscii::decode_string(&memory, address).unwrap();

        assert_eq!(name, "hi");
    }
}
