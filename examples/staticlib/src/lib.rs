const PANIC_STR: &str = "panic";

mod module;

/// A library function that can panic.
/// NOTE: For library-only analysis, jonesy detects panics at their source location
/// (in module/mod.rs), not at call sites here. This is a limitation of static
/// analysis without binary entry points - we can only detect direct panic calls.
pub fn library_function() {
    if std::env::args().len() > 1 {
        // TODO: jonesy cannot reliably detect conditional panics in library mode
        panic!("{}", PANIC_STR);
    }

    // Call site markers - jonesy detects these at their source in module/mod.rs
    module::cause_a_panic();
    module::cause_an_unwrap();
    module::cause_unwrap_err();
    module::cause_expect_none();
    module::cause_expect_err();
    module::cause_unwrap_err_on_ok();
    module::cause_expect_err_on_ok();
    module::cause_assert();
    module::cause_assert_eq();
    module::cause_assert_ne();
    module::cause_debug_assert();
    module::cause_debug_assert_eq();
    module::cause_debug_assert_ne();
    module::cause_unreachable();
    module::cause_unimplemented();
    module::cause_todo();
    module::cause_divide_by_zero();
    module::cause_arithmetic_overflow();
    module::cause_shift_overflow();
    module::cause_slice_index_oob();
    module::cause_string_index_panic();
}
