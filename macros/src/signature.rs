use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Expr, TypeBareFn};

pub(crate) fn check(source: &Expr, signature: &TypeBareFn, target: &Expr) -> TokenStream {
    let native = signature
        .abi
        .as_ref()
        .is_some_and(|abi| abi.name.as_ref().is_none_or(|name| name.value() != "Rust"));
    if signature.unsafety.is_some() || native {
        return TokenStream::new();
    }
    let args: Vec<_> = signature.inputs.iter().map(|arg| &arg.ty).collect();
    let types: Vec<_> = (0..args.len()).map(|i| format_ident!("__Arg{i}")).collect();
    let names: Vec<_> = (0..args.len())
        .map(|i| format_ident!("__arg{i}", span = Span::mixed_site()))
        .collect();
    let target_name = format_ident!("__target", span = Span::mixed_site());
    let binder = &signature.lifetimes;
    let mut probes = TokenStream::new();
    // Probe borrowed inputs without rejecting static-only functions.
    for index in 0..args.len() {
        for mutable in [false, true] {
            let name = format_ident!("__Input{index}{}", usize::from(mutable));
            let method = format_ident!("__input{index}{}", usize::from(mutable));
            let fallback = format_ident!("__Fallback{index}{}", usize::from(mutable));
            let other: Vec<_> = types
                .iter()
                .enumerate()
                .filter_map(|(i, ty)| (i != index).then_some(ty))
                .collect();
            let mutability = mutable.then(|| quote!(mut));
            let probe_args: Vec<_> = types
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    if i == index {
                        quote!(&'__probe #mutability __Pointee)
                    } else {
                        quote!(#ty)
                    }
                })
                .collect();
            probes.extend(quote! {
                trait #name<__Pointee: ?Sized, #(#other),*> {
                    fn #method<__Target>(&self, target: __Target)
                    where __Target: for<'__probe> __Signature<(#(#probe_args,)*)>;
                }
                impl<__Source, __Pointee: ?Sized, #(#other),*> #name<__Pointee, #(#other),*>
                    for __SourceValue<__Source>
                where __Source: for<'__probe> __Signature<(#(#probe_args,)*)> {
                    fn #method<__Target>(&self, _: __Target)
                    where __Target: for<'__probe> __Signature<(#(#probe_args,)*)> {}
                }
                trait #fallback { fn #method<__Target>(&self, target: __Target); }
                impl<__Source> #fallback for &__SourceValue<__Source> {
                    fn #method<__Target>(&self, _: __Target) {}
                }
                (&__SourceValue(#source)).#method(#target_name);
            });
        }
    }
    quote! {
        {
            trait __Signature<__Args> { type Output; }
            impl<__Function, __Output, #(#types),*> __Signature<(#(#types,)*)> for __Function
            where __Function: ::std::ops::FnOnce(#(#types),*) -> __Output {
                type Output = __Output;
            }
            struct __SourceValue<__Source>(__Source);
            // Mutable references keep the input lifetimes distinct.
            fn __same_output<__Source, __Target, #(#types),*>(_: __Source, _: __Target, _: (#(&mut #types,)*))
            where __Source: __Signature<(#(#types,)*)>,
                  __Target: __Signature<(#(#types,)*), Output = <__Source as __Signature<(#(#types,)*)>>::Output> {}
            #[allow(clippy::type_complexity)]
            let _: &mut dyn #binder ::std::ops::FnMut(#(#args),*) = &mut |#(mut #names),*| {
                let #target_name: #signature = #target;
                #probes
                __same_output(#source, #target_name, (#(&mut #names,)*));
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check;
    use syn::parse_quote;

    #[test]
    fn checks_safe_inputs_and_outputs() {
        for signature in [
            parse_quote!(fn() -> usize),
            parse_quote!(extern "Rust" fn(&str) -> &str),
            parse_quote!(for<'a> fn(&'a str, usize) -> &'a str),
        ] {
            let expanded = check(&parse_quote!(source), &signature, &parse_quote!(target));
            syn::parse2::<syn::Block>(quote::quote!({ #expanded })).unwrap();
            assert!(expanded.to_string().contains("__same_output"));
        }
    }

    #[test]
    fn leaves_other_function_kinds_to_pointer_checks() {
        for signature in [
            parse_quote!(unsafe fn()),
            parse_quote!(extern "C" fn()),
            parse_quote!(extern "C" fn()),
        ] {
            assert!(check(&parse_quote!(source), &signature, &parse_quote!(target)).is_empty());
        }
    }
}
