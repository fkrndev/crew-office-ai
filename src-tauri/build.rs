use std::fs;
use std::path::Path;

/// Name of the generated permission set that bundles every `allow-<command>`
/// permission for the app's own `#[tauri::command]`s. `capabilities/default.json`
/// references this single identifier instead of hand-listing every name.
const APP_COMMANDS_SET: &str = "allow-all-app-commands";

/// Locate the `tauri::generate_handler![ ... ]` invocation in `src/lib.rs` and
/// return the bare command names it registers. lib.rs is the single source of
/// truth: the ACL manifest below is derived from it, so a command can never be
/// registered without also getting an `allow-<command>` permission.
fn parse_handler_commands(src: &str) -> Result<Vec<String>, String> {
    let marker = "generate_handler!";
    let bytes = src.as_bytes();

    // Find the invocation, ignoring occurrences inside `//` line comments.
    let mut open = None;
    let mut from = 0;
    while let Some(rel) = src[from..].find(marker) {
        let at = from + rel;
        let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
        let in_line_comment = src[line_start..at].contains("//");
        from = at + marker.len();
        if in_line_comment {
            continue;
        }
        let rest = src[from..].trim_start();
        if rest.starts_with('[') {
            if open.is_some() {
                return Err("found more than one `generate_handler![` invocation".into());
            }
            open = Some(src.len() - rest.len() + 1);
        }
    }
    let start = open.ok_or("no `generate_handler![` invocation found")?;

    // Read to the matching `]`, tracking nesting (for `#[cfg(..)]` attributes)
    // and skipping `//` and `/* */` comments.
    let mut depth = 1usize;
    let mut i = start;
    let mut body: Vec<u8> = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut nest = 1;
                i += 2;
                while i < bytes.len() && nest > 0 {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        nest += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        nest -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        body.push(bytes[i]);
        i += 1;
    }
    if depth != 0 {
        return Err("unterminated `generate_handler![`".into());
    }

    // Drop `#[...]` attributes (e.g. `#[cfg(windows)]`), then split on commas.
    let mut cleaned: Vec<u8> = Vec::new();
    let mut j = 0;
    while j < body.len() {
        if body[j] == b'#' {
            let mut k = j + 1;
            while k < body.len() && body[k].is_ascii_whitespace() {
                k += 1;
            }
            if body.get(k) == Some(&b'[') {
                let mut d = 0;
                while k < body.len() {
                    if body[k] == b'[' {
                        d += 1;
                    } else if body[k] == b']' {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                j = k + 1;
                continue;
            }
        }
        cleaned.push(body[j]);
        j += 1;
    }
    let cleaned = String::from_utf8(cleaned).map_err(|e| e.to_string())?;

    let mut out: Vec<String> = Vec::new();
    for item in cleaned.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let name = item.rsplit("::").next().unwrap_or(item).trim();
        let valid = !name.is_empty()
            && !name.as_bytes()[0].is_ascii_digit()
            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
        if !valid {
            return Err(format!("cannot parse command name from `{item}`"));
        }
        if out.iter().any(|n| n == name) {
            return Err(format!("command `{name}` is registered twice"));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

/// Write `contents` only if it differs, so an unchanged build does not bump the
/// mtime of `permissions/` and retrigger tauri-build's rerun-if-changed.
fn write_if_changed(path: &Path, contents: &str) {
    if fs::read_to_string(path).ok().as_deref() != Some(contents) {
        fs::write(path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }
}

/// Generate `permissions/app-commands.toml`: one permission *set* holding every
/// `allow-<command>`. tauri-build itself autogenerates the individual
/// `allow-*`/`deny-*` permissions under `permissions/autogenerated/`; the
/// capability grants this one set. Also prunes autogenerated files for commands
/// that were removed from lib.rs.
fn write_permission_set(commands: &[String]) {
    let perms_dir = Path::new("permissions");
    fs::create_dir_all(perms_dir).expect("create permissions dir");

    let auto_dir = perms_dir.join("autogenerated");
    if let Ok(entries) = fs::read_dir(&auto_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if p.extension().and_then(|s| s.to_str()) == Some("toml")
                && !commands.iter().any(|c| c == stem)
            {
                let _ = fs::remove_file(&p);
            }
        }
    }

    let mut toml = String::from(
        "# Automatically generated by build.rs from `tauri::generate_handler![..]` in\n\
         # src/lib.rs - DO NOT EDIT!\n\n\
         [[set]]\n",
    );
    toml.push_str(&format!("identifier = \"{APP_COMMANDS_SET}\"\n"));
    toml.push_str(
        "description = \"Allows the webview to call every custom app command registered in lib.rs.\"\n",
    );
    toml.push_str("permissions = [\n");
    for c in commands {
        toml.push_str(&format!("  \"allow-{}\",\n", c.replace('_', "-")));
    }
    toml.push_str("]\n");
    write_if_changed(&perms_dir.join("app-commands.toml"), &toml);
}

fn main() {
    // The default manifest arrives as an embedded winres resource — which
    // only reaches BINS. The `cargo test` lib harness is not a bin, so it
    // binds comctl32 v5 and dies at load with STATUS_ENTRYPOINT_NOT_FOUND
    // on the comctl6-only TaskDialogIndirect import that the dialog/tao code
    // path pulls in (tauri-apps/tauri#13419). Emit the Common-Controls v6
    // dependency through the LINKER instead — it reaches every linked
    // product, test harnesses included.
    #[cfg(windows)]
    {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='Win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    // ACL: every custom `#[tauri::command]` needs an `allow-<command>`
    // permission granted by capabilities/default.json (via the generated
    // `allow-all-app-commands` set). The command list is parsed out of
    // src/lib.rs so it cannot drift from `generate_handler!`.
    println!("cargo:rerun-if-changed=src/lib.rs");
    let lib_rs = fs::read_to_string("src/lib.rs").expect("read src/lib.rs");
    let commands = parse_handler_commands(&lib_rs).unwrap_or_else(|e| {
        panic!("build.rs: failed to parse generate_handler! in src/lib.rs: {e}")
    });
    if commands.is_empty() {
        panic!(
            "build.rs: generate_handler![] in src/lib.rs lists no commands; \
             refusing to build an ACL manifest that would lock the webview out of every command"
        );
    }
    write_permission_set(&commands);
    // AppManifest::commands wants a 'static slice; the build script is
    // short-lived, so leaking is fine.
    let names: Vec<&'static str> = commands
        .into_iter()
        .map(|c| &*Box::leak(c.into_boxed_str()))
        .collect();
    let names: &'static [&'static str] = Box::leak(names.into_boxed_slice());

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
            .app_manifest(tauri_build::AppManifest::new().commands(names)),
    )
    .expect("tauri build script");
}
