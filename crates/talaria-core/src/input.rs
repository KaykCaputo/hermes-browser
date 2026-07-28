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
