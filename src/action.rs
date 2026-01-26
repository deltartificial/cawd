/// Actions that can be performed in the application
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Action {
    Quit,
    SwitchPanel,
    Up,
    Down,
    Left,
    Right,
    Enter,
    PageUp,
    PageDown,
    Home,
    End,
    FileSelected(std::path::PathBuf),
    ToggleHidden,
    EnterSearchMode,
    ExitSearchMode,
    SearchInput(char),
    SearchBackspace,
    SearchConfirm,
    None,
}
