/// The mouse cursor a page asks for: one variant per CSS cursor keyword the engine reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PointerCursor {
    #[default]
    Default,
    Pointer,
    Text,
    Wait,
    Progress,
    Crosshair,
    Move,
    NotAllowed,
    Grab,
    Grabbing,
    Help,
    ContextMenu,
    Cell,
    VerticalText,
    Alias,
    Copy,
    NoDrop,
    AllScroll,
    ColResize,
    RowResize,
    NResize,
    EResize,
    SResize,
    WResize,
    NeResize,
    NwResize,
    SeResize,
    SwResize,
    EwResize,
    NsResize,
    NeswResize,
    NwseResize,
    ZoomIn,
    ZoomOut,
    /// The page hides the cursor.
    None,
}

/// The colour scheme pages see through `prefers-color-scheme`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorScheme {
    #[default]
    Light,
    Dark,
}
