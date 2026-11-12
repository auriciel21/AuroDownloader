//! Plugins d'essai (SPEC §7.20), écrits en texte WebAssembly (`wat`) : allocateur simple,
//! textes rangés en mémoire, corps de `auro_resolve` au choix ; manifeste ajouté en section
//! personnalisée. Pour les tests du moteur et ceux de la CLI.

/// Une chaîne de texte WebAssembly.
fn wat_string(text: &str) -> String {
    let mut out = String::from("\"");
    for b in text.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:02x}")),
        }
    }
    out.push('"');
    out
}

/// Ajoute une section personnalisée à un module.
pub fn with_section(mut wasm: Vec<u8>, name: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![name.len() as u8];
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);
    wasm.push(0);
    let mut size = body.len();
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        wasm.push(if size > 0 { byte | 0x80 } else { byte });
        if size == 0 {
            break;
        }
    }
    wasm.extend(body);
    wasm
}

/// Manifeste d'essai : site et domaine `fichiers.exemple`.
pub const MANIFEST: &str = r#"{"abi": 1, "id": "org.essai.fichiers", "name": "Fichiers d'essai",
    "version": "1.0", "sites": ["fichiers.exemple"],
    "fetch": ["fichiers.exemple"]}"#;

/// Un plugin d'essai : `data` (textes rangés à partir de l'adresse 1024, 1024 octets
/// chacun), corps de `auro_resolve` (`$ptr`, `$len` → i64), autres définitions.
pub fn plugin(manifest: &str, data: &[&str], resolve: &str, extra: &str) -> Vec<u8> {
    let segments: String = data
        .iter()
        .enumerate()
        .map(|(i, text)| {
            format!(
                "(data (i32.const {}) {})\n",
                1024 * (i + 1),
                wat_string(text)
            )
        })
        .collect();
    let text = format!(
        r#"(module
  (import "auro" "fetch" (func $fetch (param i32 i32) (result i64)))
  (import "auro" "log" (func $log (param i32 i32)))
  (memory (export "memory") 4)
  (global $heap (mut i32) (i32.const 131072))
  (func $alloc (export "auro_alloc") (param $n i32) (result i32)
    (local $at i32)
    (local.set $at (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (block $ok
      (br_if $ok (i32.le_u (global.get $heap) (i32.mul (memory.size) (i32.const 65536))))
      (drop (memory.grow (i32.add (i32.shr_u (local.get $n) (i32.const 16)) (i32.const 1)))))
    (local.get $at))
  {segments}
  (func (export "auro_resolve") (param $ptr i32) (param $len i32) (result i64) {resolve})
  {extra}
)"#
    );
    let wasm = wat::parse_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    with_section(wasm, crate::plugins::manifest::SECTION, manifest.as_bytes())
}

/// Le texte n° `i` de `data` : (adresse << 32) | taille.
pub fn packed(i: usize, text: &str) -> String {
    format!(
        "(i64.or (i64.shl (i64.const {}) (i64.const 32)) (i64.const {}))",
        1024 * (i + 1),
        text.len()
    )
}

/// Appel de `fetch` sur le texte n° `i`.
pub fn fetch_call(i: usize, text: &str) -> String {
    format!(
        "(call $fetch (i32.const {}) (i32.const {}))",
        1024 * (i + 1),
        text.len()
    )
}

/// Un plugin qui rend un texte fixe.
pub fn fixed(output: &str) -> Vec<u8> {
    plugin(MANIFEST, &[output], &packed(0, output), "")
}
