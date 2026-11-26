//! A check that a uniform written twice, as a Rust struct and as WGSL, has
//! the same fields in the same order with the same types, for each crate's
//! tests.

/// The fields of `struct name { … }` in `source`, as (name, type) in
/// order, comments dropped and `pub` stripped.
pub fn struct_fields(source: &str, name: &str) -> Vec<(String, String)> {
    let start = source.find(&format!("struct {name} {{")).expect("struct present");
    let body = &source[start..];
    let body = &body[body.find('{').unwrap() + 1..body.find('}').unwrap()];
    body.lines()
        .map(|l| l.split("//").next().unwrap().trim().trim_end_matches(','))
        .filter(|l| !l.is_empty())
        .map(|l| {
            let l = match l.strip_prefix("pub(") {
                Some(rest) => rest.split_once(") ").expect("pub(…) field").1,
                None => l.trim_start_matches("pub "),
            };
            let (n, t) = l.split_once(':').expect("name: type");
            (n.trim().to_owned(), t.trim().to_owned())
        })
        .collect()
}

/// `rust`, a Rust field type of a uniform, as WGSL writes it.
pub fn wgsl_type(rust: &str) -> &'static str {
    match rust {
        "glam::Vec4" | "Vec4" => "vec4f",
        "UVec4" => "vec4u",
        "u32" => "u32",
        "f32" => "f32",
        other => panic!("no WGSL type for {other}"),
    }
}

/// Panics unless each of `names` has the same fields in `rust` and `wgsl`.
pub fn assert_mirrors(rust: &str, wgsl: &str, names: &[&str]) {
    for name in names {
        let want: Vec<(String, String)> = struct_fields(rust, name)
            .into_iter()
            .map(|(n, t)| (n, wgsl_type(&t).to_owned()))
            .collect();
        assert!(!want.is_empty(), "{name} has fields");
        assert_eq!(struct_fields(wgsl, name), want, "{name}");
    }
}
