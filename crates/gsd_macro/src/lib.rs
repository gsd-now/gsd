mod gsd_task_macro;

use proc_macro::TokenStream;

#[proc_macro_derive(GsdTask, attributes(gsd_task))]
pub fn gsd_task_derive(input: TokenStream) -> TokenStream {
    gsd_task_macro::gsd_task_macro(input)
}
