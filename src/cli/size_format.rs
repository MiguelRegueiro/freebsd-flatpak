pub(super) fn format(size: u64) -> String {
    glib::format_size(size).replace('\u{a0}', " ")
}

#[cfg(test)]
#[path = "tests/size_format.rs"]
mod tests;
