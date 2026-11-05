//! The texts that gaP carries: the help, the template of the `.ini` and the license.
//!
//! They are stored deflated. `build.rs` compresses them at every build, from the files in the repository, and this
//! module inflates them when they are needed, so the program is smaller and a text that is edited is never out of date.

static HELP_Z: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gaP_Help.z"));
static INI_Z: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gaP.ini.z"));
static LICENSE_Z: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/LICENSE.z"));

fn inflate(z: &[u8]) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec(z).ok()
}

/// `res/gaP_Help.txt`, byte by byte. `None` = the embedded data is damaged.
pub fn help() -> Option<Vec<u8>> {
    inflate(HELP_Z)
}

/// `res/gaP.ini`, byte by byte. `None` = the embedded data is damaged.
pub fn ini_template() -> Option<Vec<u8>> {
    inflate(INI_Z)
}

/// `LICENSE`. `None` = the embedded data is damaged.
pub fn license() -> Option<String> {
    String::from_utf8(inflate(LICENSE_Z)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_texts_are_the_files_of_the_repository() {
        assert_eq!(help().as_deref(), Some(&include_bytes!("../res/gaP_Help.txt")[..]));
        assert_eq!(ini_template().as_deref(), Some(&include_bytes!("../res/gaP.ini")[..]));
        assert_eq!(license().as_deref(), Some(include_str!("../LICENSE")));
    }

    #[test]
    fn damaged_data_is_refused() {
        assert!(inflate(&[0xFF, 0xFF, 0xFF, 0xFF]).is_none());
        assert!(inflate(&HELP_Z[..HELP_Z.len() / 2]).is_none());
    }
}
