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
        quote!(shimforge, session, source, unsafe fn()),
        quote!(shimforge, session, source, extern "C" fn()),
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
