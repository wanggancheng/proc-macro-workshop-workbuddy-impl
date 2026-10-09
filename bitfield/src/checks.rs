//! Compile-time assertions that require type information, implemented by
//! leaning on the trait system.
//!
//! A procedural macro cannot look up the value of a `const` item or an
//! associated constant, and it cannot compute the width in bits of a field
//! whose type is written as a type alias or an enum. What it *can* do is emit
//! code that the compiler type-checks, which is enough to turn "this
//! expression must equal 0" into a trait bound error.

/// Satisfied only by bitfield sizes that are a multiple of 8 bits.
pub trait TotalSizeIsMultipleOfEightBits {}

/// Satisfied only by in-range enum discriminants.
pub trait DiscriminantInRange {}

pub struct ZeroMod8;
pub struct OneMod8;
pub struct TwoMod8;
pub struct ThreeMod8;
pub struct FourMod8;
pub struct FiveMod8;
pub struct SixMod8;
pub struct SevenMod8;

impl TotalSizeIsMultipleOfEightBits for ZeroMod8 {
}

pub struct True;
pub struct False;

impl DiscriminantInRange for True {
}

/// Maps a constant `usize` (used as an array length) to a type, so that a
/// procedural macro can select a type based on a value the compiler computes.
pub trait TypeArray {
    type Type;
}

impl TypeArray for [(); 0] {
    type Type = ZeroMod8;
}

impl TypeArray for [(); 1] {
    type Type = OneMod8;
}

impl TypeArray for [(); 2] {
    type Type = TwoMod8;
}

impl TypeArray for [(); 3] {
    type Type = ThreeMod8;
}

impl TypeArray for [(); 4] {
    type Type = FourMod8;
}

impl TypeArray for [(); 5] {
    type Type = FiveMod8;
}

impl TypeArray for [(); 6] {
    type Type = SixMod8;
}

impl TypeArray for [(); 7] {
    type Type = SevenMod8;
}

/// Maps a constant `0`/`1` (used as an array length) to `True` or `False`.
pub trait InRangeArray {
    type Type;
}

impl InRangeArray for [(); 0] {
    type Type = False;
}

impl InRangeArray for [(); 1] {
    type Type = True;
}
