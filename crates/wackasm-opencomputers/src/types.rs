//! Enums for the numbers OpenComputers uses as codes.
//! They convert into [`Number`], so they can be passed wherever a method
//! takes a side or colour (and into `Value` through it).

use crate::value::Number;

/// A side, as robots, drones, redstone and transposers number them.
/// Relative names apply to robots and drones; absolute ones to blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Side {
    /// Down / bottom.
    Bottom = 0,
    /// Up / top.
    Top = 1,
    /// North, or back for robots.
    Back = 2,
    /// South, or front for robots.
    Front = 3,
    /// West, or right for robots.
    Right = 4,
    /// East, or left for robots.
    Left = 5,
}

impl Side {
    pub const DOWN: Side = Side::Bottom;
    pub const UP: Side = Side::Top;
    pub const NORTH: Side = Side::Back;
    pub const SOUTH: Side = Side::Front;
    pub const WEST: Side = Side::Right;
    pub const EAST: Side = Side::Left;
    pub const ALL: [Side; 6] = [Side::Bottom, Side::Top, Side::Back, Side::Front, Side::Right, Side::Left];

    pub fn from_index(i: i64) -> Option<Side> {
        Side::ALL.get(usize::try_from(i).ok()?).copied()
    }
}

impl From<Side> for Number {
    fn from(side: Side) -> Number {
        Number::Int(side as i64)
    }
}

/// A bundled-redstone / dye colour index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Color {
    White = 0,
    Orange = 1,
    Magenta = 2,
    LightBlue = 3,
    Yellow = 4,
    Lime = 5,
    Pink = 6,
    Gray = 7,
    Silver = 8,
    Cyan = 9,
    Purple = 10,
    Blue = 11,
    Brown = 12,
    Green = 13,
    Red = 14,
    Black = 15,
}

impl From<Color> for Number {
    fn from(color: Color) -> Number {
        Number::Int(color as i64)
    }
}

/// Keyboard codes (`Signal::KeyDown::code`), for keys without a character.
pub mod keys {
    pub const ESCAPE: u32 = 1;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const ENTER: u32 = 28;
    pub const LEFT_CONTROL: u32 = 29;
    pub const LEFT_SHIFT: u32 = 42;
    pub const SPACE: u32 = 57;
    pub const HOME: u32 = 199;
    pub const UP: u32 = 200;
    pub const PAGE_UP: u32 = 201;
    pub const LEFT: u32 = 203;
    pub const RIGHT: u32 = 205;
    pub const END: u32 = 207;
    pub const DOWN: u32 = 208;
    pub const PAGE_DOWN: u32 = 209;
    pub const INSERT: u32 = 210;
    pub const DELETE: u32 = 211;
}
