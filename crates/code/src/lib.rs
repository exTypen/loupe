//! Code understanding for Loupe: syntax, code index, SQL/YAML structure, change lens, file icons. No UI.

pub mod git;
pub mod icons;
pub mod index;
pub mod inspector;
pub mod lens;
pub mod overview;
pub mod sql;
pub mod syntax;
pub mod theme;
pub mod yaml;

/// Path pieces as NSString computed them, which the Swift logic was written against.
pub(crate) mod path {
    /// The last component.
    pub fn file_name(p: &str) -> &str {
        let p = p.trim_end_matches('/');
        p.rsplit('/').next().unwrap_or(p)
    }

    /// After the last dot of the last component ("" without one); ".envrc" gives "envrc".
    pub fn extension(p: &str) -> &str {
        file_name(p).rsplit_once('.').map_or("", |(_, e)| e)
    }

    /// Everything before the last component: "a/b" → "a", "a" → "", "/a" → "/".
    pub fn parent(p: &str) -> &str {
        let p = p.trim_end_matches('/');
        match p.rfind('/') {
            Some(0) => "/",
            Some(i) => &p[..i],
            None => "",
        }
    }

    /// The path without its extension.
    pub fn without_extension(p: &str) -> &str {
        let ext = extension(p);
        if ext.is_empty() { p } else { &p[..p.len() - ext.len() - 1] }
    }
}
pub(crate) use path::{extension, file_name, parent};
