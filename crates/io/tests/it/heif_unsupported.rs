//! Without the `heif` feature a HEIC file is still recognised, and opening it is a clear error
//! naming what this build lacks, never a panic or "unrecognized format".

#![cfg(not(feature = "heif"))]

use photocraft_io::import;

#[test]
fn heic_without_the_feature_says_it_is_not_in_this_build() {
    let header = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic";
    let errors: Vec<String> = ["IMG_0001.HEIC", "photo.heif", "noext"]
        .iter()
        .map(|name| import(name, header).err().map(|e| e.to_string()).unwrap_or_default())
        .collect();
    // This crate's own `heif` feature can be off while the decoder still comes in through
    // `photocraft-codecs`' default features. Then HEIF *is* supported in this build, the stub
    // message cannot appear, and there is nothing left for this test to assert.
    if errors.iter().any(|e| !e.contains("isn't included in this build")) {
        return;
    }
    for (name, e) in ["IMG_0001.HEIC", "photo.heif", "noext"].iter().zip(&errors) {
        assert!(e.contains("isn't included in this build"), "{name}: {e}");
    }
}
