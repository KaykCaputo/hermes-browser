#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLocation {
    Standard,
    Left,
    Right,
    Numpad,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyboardModifiers {
    pub alt: bool,
    pub control: bool,
    pub meta: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardInput {
    pub state: KeyState,
    pub key: String,
    pub code: String,
    pub location: KeyLocation,
    pub modifiers: KeyboardModifiers,
    pub repeat: bool,
    pub is_composing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionState {
    Start,
    Update,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeInput {
    Composition {
        state: CompositionState,
        data: String,
    },
    Dismissed,
}

/// A clipboard edit performed on whatever has focus inside the page, as if its key had been
/// pressed: Servo runs the same editing code for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardCommand {
    Cut,
    Copy,
    Paste,
}
