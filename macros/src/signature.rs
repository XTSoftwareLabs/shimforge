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
    // A borrow of this local cannot last for 'static. `&()` would be promoted to a
    // 'static borrow and accept every signature.
    let local = format_ident!(
        "borrowed_inputs_must_accept_any_lifetime",
        span = Span::mixed_site()
    );
    let binder = &signature.lifetimes;
    let mut probes = TokenStream::new();
    // Every borrowed input of the declared signature must accept a borrow of a local
    // value, so a rule never treats a short borrow as a longer one. The probe impl is
    // chosen by the input's type alone and the local borrow is checked afterwards, so
    // no lifetime decides which impl applies and every trait solver agrees.
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
            let borrowed = |lifetime: TokenStream| -> Vec<TokenStream> {
                types
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| {
                        if i == index {
                            quote!(&#lifetime #mutability __Pointee)
                        } else {
                            quote!(#ty)
                        }
                    })
                    .collect()
            };
            let shape_args = borrowed(quote!('__shape));
            let local_args = borrowed(quote!('__local));
            probes.extend(quote! {
                trait #name<'__shape, __Pointee: ?Sized, #(#other),*> {
                    fn #method<'__local, __Target>(&self, target: __Target, local: &'__local ())
                    where __Pointee: '__local, __Target: __Signature<(#(#local_args,)*)>;
                }
                impl<'__shape, __Shape, __Pointee: ?Sized + '__shape, #(#other),*> #name<'__shape, __Pointee, #(#other),*>
                    for __Declared<__Shape>
                where __Shape: __Signature<(#(#shape_args,)*)> {
                    fn #method<'__local, __Target>(&self, _: __Target, _: &'__local ())
                    where __Pointee: '__local, __Target: __Signature<(#(#local_args,)*)> {}
                }
                trait #fallback { fn #method<__Target>(&self, target: __Target, local: &()); }
                impl<__Shape> #fallback for &__Declared<__Shape> {
                    fn #method<__Target>(&self, _: __Target, _: &()) {}
                }
                (&__Declared(#target_name)).#method(#target_name, &#local);
            });
        }
    }
    let local_value = (!args.is_empty()).then(|| quote!(let #local = ();));
    quote! {
        {
            trait __Signature<__Args> { type Output; }
            impl<__Function, __Output, #(#types),*> __Signature<(#(#types,)*)> for __Function
            where __Function: ::std::ops::FnOnce(#(#types),*) -> __Output {
                type Output = __Output;
            }
            struct __Declared<__Shape>(__Shape);
            // Mutable references keep the input lifetimes distinct.
            fn __same_output<__Source, __Target, #(#types),*>(_: __Source, _: __Target, _: (#(&mut #types,)*))
            where __Source: __Signature<(#(#types,)*)>,
                  __Target: __Signature<(#(#types,)*), Output = <__Source as __Signature<(#(#types,)*)>>::Output> {}
            #[allow(clippy::type_complexity)]
            let _: &mut dyn #binder ::std::ops::FnMut(#(#args),*) = &mut |#(mut #names),*| {
                let #target_name: #signature = #target;
                #local_value
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
