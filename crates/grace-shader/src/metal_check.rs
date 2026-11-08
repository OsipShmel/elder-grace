//! Native `.metallib` check: load into an MTLDevice and create every function.

use std::path::Path;

use anyhow::{Context, Result};
use objc2_foundation::{NSString, NSURL};
use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice, MTLFunction, MTLLibrary};

pub fn run(dir: &Path) -> Result<()> {
    let device = MTLCreateSystemDefaultDevice().context("no Metal device")?;
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("metallib"))
        .collect();
    files.sort();
    let (mut libs_ok, mut funcs_ok, mut failed) = (0, 0, 0);
    for path in &files {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match device.newLibraryWithURL_error(&url) {
            Ok(lib) => {
                libs_ok += 1;
                let names = lib.functionNames();
                let mut created = Vec::new();
                for i in 0..names.count() {
                    let fname = names.objectAtIndex(i);
                    if let Some(f) = lib.newFunctionWithName(&fname) {
                        funcs_ok += 1;
                        created.push(format!("{} ({:?})", f.name(), f.functionType()));
                    }
                }
                println!("{name}: {}", created.join(", "));
            }
            Err(e) => {
                failed += 1;
                println!("{name}: ERROR {}", e.localizedDescription());
            }
        }
    }
    println!(
        "Metal ({}): libraries loaded {libs_ok}/{}, functions created {funcs_ok}, errors {failed}",
        device.name(),
        files.len()
    );
    Ok(())
}
