use super::*;

fn expanded(input: Tokens) -> String {
    let tokens = expand(syn::parse2(input));
    syn::parse2::<Expr>(tokens.clone()).unwrap();
    tokens.to_string()
}

#[test]
fn owned_outputs_have_value_helpers() {
    let tokens = expanded(quote!(
        ::renamed,
        session,
        source,
        fn(u32, String) -> String,
    ));
    assert!(tokens.contains("pub fn returns_default"));
    assert!(tokens.contains("pub fn return_once"));
    assert!(tokens.contains(":: renamed :: __install"));
}

#[test]
fn unit_output_and_empty_arguments() {
    let tokens = expanded(quote!(shimforge, session, source, fn()));
    assert!(tokens.contains("__ShimforgeBuilder < () >"));
}

#[test]
fn borrowed_outputs_keep_their_lifetimes() {
    for signature in [
        quote!(fn(&str) -> &str),
        quote!(for<'a> fn(&'a mut [u8]) -> &'a mut [u8]),
        quote!(for<'a> fn(&'a str) -> std::borrow::Cow<'a, str>),
    ] {
        let tokens = expanded(quote!(shimforge, session, source, #signature));
        assert!(!tokens.contains("pub fn returns_default"));
        assert!(tokens.contains("pub fn returning_once"));
    }
}

#[test]
fn static_output_has_value_helpers() {
    let tokens = expanded(quote!(shimforge, session, source, fn() -> &'static str));
    assert!(tokens.contains("pub fn returns"));
}

#[test]
fn unsupported_signatures_and_invalid_input_report_errors() {
    for input in [
        quote!(shimforge, session, source, fn(...)),
        quote!(shimforge),
        quote!(shimforge, session, source, fn(), extra),
    ] {
        assert!(
            expand(syn::parse2(input))
                .to_string()
                .contains("compile_error")
        );
    }
}

#[test]
fn native_and_unsafe_signatures_keep_their_abi() {
    for signature in [
        quote!(unsafe fn(i32) -> i32),
        quote!(extern "C" fn(i32) -> i32),
        quote!(unsafe extern "system" fn(i32) -> i32),
        quote!(extern "C-unwind" fn(i32) -> i32),
    ] {
        let tokens = expanded(quote!(shimforge, session, source, #signature));
        assert!(tokens.contains("pub fn with"));
        assert!(tokens.contains("pub fn times"));
    }
}

#[test]
fn replacement_checks_parse_the_source_target_and_signature() {
    let checked = expand_check(syn::parse2(quote!(source, saved, target, fn(&str) -> &str)));
    assert!(checked.to_string().contains("__same_output"));
    let invalid = expand_check(syn::parse2(quote!(source)));
    assert!(invalid.to_string().contains("compile_error"));
}

#[test]
fn source_paths_keep_their_generic_lifetimes() {
    let saved = parse_quote!(saved);
    for source in [parse_quote!(source), parse_quote!((source))] {
        let checked = source_item(&source, &saved);
        assert_eq!(quote!(#checked).to_string(), "source");
    }
    let grouped = Expr::Group(syn::ExprGroup {
        attrs: Vec::new(),
        group_token: Default::default(),
        expr: Box::new(parse_quote!(source)),
    });
    assert!(matches!(source_item(&grouped, &saved), Expr::Path(_)));
    let expression = parse_quote!(choose()?);
    assert!(std::ptr::eq(source_item(&expression, &saved), &saved));
}
