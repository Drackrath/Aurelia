//! Release number and codename.

/// Codename literal for `concat!`.
macro_rules! codename {
    () => {
        "Asylum Demon"
    };
}

/// Souls boss naming this release line.
pub const CODENAME: &str = codename!();

/// Bare number; compare versions with this.
pub const NUMBER: &str = env!("CARGO_PKG_VERSION");

/// Number, then codename; `--version` output.
pub const DISPLAY: &str = concat!(env!("CARGO_PKG_VERSION"), " ", codename!());

#[cfg(test)]
mod tests;
