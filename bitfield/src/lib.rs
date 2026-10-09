// Crates that have the "proc-macro" crate type are only allowed to export
// procedural macros. So we cannot have one crate that defines procedural macros
// alongside other types of public APIs like traits and structs.
//
// For this project we are going to need a #[bitfield] macro but also a trait
// and some structs. We solve this by defining the trait and structs in this
// crate, defining the attribute macro in a separate bitfield-impl crate, and
// then re-exporting the macro from this crate so that users only have one crate
// that they need to import.
//
// From the perspective of a user of this crate, they get all the necessary APIs
// (macro, trait, struct) through the one bitfield crate.
pub use bitfield_impl::bitfield;
pub use bitfield_impl::define_specifiers;
pub use bitfield_impl::BitfieldSpecifier;

pub mod checks;

/// Types that can be used as the type of a `#[bitfield]` struct field.
pub trait Specifier {
    /// Number of bits occupied by this field type.
    const BITS: usize;

    /// The integer (or enum, or `bool`) type that callers use to read and
    /// write a field of this type.
    type Accessor;

    /// Convert a value of the accessor type into the raw bit representation.
    fn into_bits(value: Self::Accessor) -> u64;

    /// Convert a raw bit representation back into the accessor type.
    fn from_bits(bits: u64) -> Self::Accessor;
}

/// `bool` occupies exactly one bit in a bitfield.
impl Specifier for bool {
    const BITS: usize = 1;

    type Accessor = bool;

    fn into_bits(value: bool) -> u64 {
        value as u64
    }

    fn from_bits(bits: u64) -> bool {
        bits != 0
    }
}

// Defines B1 through B64 and their Specifier implementations.
define_specifiers!();
