boc.pdf: boc.typ sections/example.pdf sections/scheduled.pdf
	typst compile boc.typ boc-part.pdf
	pdftk A=boc-part.pdf B=sections/example.pdf C=sections/scheduled.pdf cat A1-17 B A18 C A19-end output boc.pdf
	rm -f boc-part.pdf

pdf: boc.pdf

test:
	cargo nextest run --release

clean:
	cargo clean
	rm -f boc.pdf
	rm -f boc-part.pdf

clippy:
	cargo clippy -- \
    -D clippy::all \
    -W clippy::pedantic \
    -A clippy::module_name_repetitions \
    -D clippy::as_ptr_cast_mut \
    -D clippy::branches_sharing_code \
    -D clippy::clear_with_drain \
    -D clippy::collection_is_never_read \
    -D clippy::debug_assert_with_mut_call \
    -D clippy::derive_partial_eq_without_eq \
    -D clippy::empty_line_after_doc_comments \
    -D clippy::empty_line_after_outer_attr \
    -D clippy::equatable_if_let \
    -D clippy::fallible_impl_from \
    -D clippy::future_not_send \
    -D clippy::imprecise_flops \
    -D clippy::iter_on_empty_collections \
    -D clippy::iter_with_drain \
    -D clippy::large_stack_frames \
    -D clippy::mutex_integer \
    -D clippy::needless_collect \
    -D clippy::needless_pass_by_ref_mut \
    -D clippy::nonstandard_macro_braces \
    -D clippy::path_buf_push_overwrite \
    -D clippy::read_zero_byte_vec \
    -D clippy::redundant_pub_crate \
    -D clippy::set_contains_or_insert \
    -D clippy::significant_drop_in_scrutinee \
    -D clippy::significant_drop_tightening \
    -D clippy::suspicious_operation_groupings \
    -D clippy::trailing_empty_array \
    -D clippy::trait_duplication_in_bounds \
    -D clippy::transmute_undefined_repr \
    -D clippy::trivial_regex \
    -D clippy::type_repetition_in_bounds \
    -D clippy::uninhabited_references \
    -D clippy::unnecessary_struct_initialization \
    -D clippy::unused_peekable \
    -D clippy::unused_rounding \
    -D clippy::useless_let_if_seq \
    -W clippy::cognitive_complexity \
    -W clippy::iter_on_single_items \
    -W clippy::missing_const_for_fn \
    -W clippy::or_fun_call \
    -W clippy::suboptimal_flops \
    -D clippy::use_self \
    -A clippy::non_send_fields_in_send_ty \
    -A clippy::tuple_array_conversions \
    -A clippy::while_float \
    -A clippy::option_if_let_else \
    -A clippy::redundant_clone \
    -A clippy::string_lit_as_bytes \
    -D clippy::allow_attributes \
    -D clippy::allow_attributes_without_reason \
    -D clippy::clone_on_ref_ptr \
    -D clippy::default_union_representation \
    -D clippy::empty_drop \
    -D clippy::error_impl_error \
    -D clippy::filetype_is_file \
    -D clippy::format_push_string \
    -D clippy::if_then_some_else_none \
    -D clippy::infinite_loop \
    -D clippy::lossy_float_literal \
    -D clippy::mem_forget \
    -D clippy::mixed_read_write_in_expression \
    -D clippy::modulo_arithmetic \
    -D clippy::multiple_unsafe_ops_per_block \
    -D clippy::pattern_type_mismatch \
    -D clippy::rc_buffer \
    -D clippy::rc_mutex \
    -D clippy::same_name_method \
    -D clippy::str_to_string \
    -D clippy::string_add \
    -D clippy::string_to_string \
    -D clippy::try_err \
    -D clippy::unneeded_field_pattern \
    -D clippy::unused_result_ok \
    -D clippy::verbose_file_reads \
    -W clippy::decimal_literal_representation \
    -W clippy::undocumented_unsafe_blocks \
    -D clippy::negative_feature_names \
    -D clippy::redundant_feature_names \
    -D clippy::wildcard_dependencies

.PHONY: clippy test clean pdf