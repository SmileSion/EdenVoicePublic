//! Run `cargo test --release` on Linux with GLib development headers installed.
//! Optimizations are necessary to expose the original immutable-out-pointer bug.

#[cfg(test)]
mod tests {
    use glib::variant::ToVariant;

    #[test]
    fn string_variant_iterators_preserve_forward_backward_and_skipping_reads() {
        let value = ["first", "middle", "last"].to_variant();
        let mut iter = value.array_iter_str().unwrap();
        assert_eq!(iter.next(), Some("first"));
        assert_eq!(iter.next_back(), Some("last"));
        assert_eq!(iter.next(), Some("middle"));
        assert_eq!(iter.next(), None);
        assert_eq!(value.array_iter_str().unwrap().nth(1), Some("middle"));
        assert_eq!(value.array_iter_str().unwrap().nth_back(1), Some("middle"));
        assert_eq!(value.array_iter_str().unwrap().last(), Some("last"));
    }
}
