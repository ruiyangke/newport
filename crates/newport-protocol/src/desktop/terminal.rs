//! Typed desktop terminal contracts.
#[derive(Clone, Debug)]
pub struct HeldShell {
    pub terminal: uuid::Uuid,
    pub name: Option<String>,
    pub title: Option<String>,
    pub creator: crate::terminal::Computer,
    pub attached: Vec<crate::terminal::Attached>,
    pub started: u64,
    pub exited: bool,
}

#[derive(Clone, Debug)]
pub struct ShellList {
    pub shells: Vec<crate::desktop::terminal::HeldShell>,
}

#[derive(Clone, Debug)]
pub struct HeldShells {
    pub desktop: String,
    pub holds: bool,
    pub shells: Option<crate::desktop::terminal::ShellList>,
}

#[derive(Clone, Debug)]
pub struct Renamed {
    pub renamed: bool,
}

#[derive(Clone, Debug)]
pub struct Ended {
    pub ended: bool,
}

#[derive(Clone, Debug)]
pub struct CloseArgs {
    pub session: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ListArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct RenameArgs {
    pub server: uuid::Uuid,
    pub terminal: uuid::Uuid,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct EndArgs {
    pub server: uuid::Uuid,
    pub terminal: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Close(Box<crate::desktop::terminal::CloseArgs>),
    List(Box<crate::desktop::terminal::ListArgs>),
    Rename(Box<crate::desktop::terminal::RenameArgs>),
    End(Box<crate::desktop::terminal::EndArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Close(Box<crate::common::Present>),
    List(Box<crate::desktop::terminal::HeldShells>),
    Rename(Box<crate::desktop::terminal::Renamed>),
    End(Box<crate::desktop::terminal::Ended>),
}
