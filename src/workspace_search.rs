pub(crate) fn match_rank(query: &str, text: &str) -> Option<(u8, usize)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some((0, 0));
    }
    let haystack = text.to_lowercase();
    let terms = query.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        return Some((0, 0));
    }

    if terms.iter().all(|needle| {
        haystack
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| word.starts_with(needle))
    }) {
        return Some((0, match_position_sum(&terms, &haystack)));
    }

    if terms.iter().all(|needle| haystack.contains(needle)) {
        return Some((1, match_position_sum(&terms, &haystack)));
    }

    if terms
        .iter()
        .all(|needle| needle.chars().count() > 1 && fuzzy_match(needle, &haystack))
    {
        return Some((2, usize::MAX / 2));
    }

    None
}

fn match_position_sum(terms: &[&str], haystack: &str) -> usize {
    terms
        .iter()
        .filter_map(|needle| haystack.find(needle))
        .sum()
}

fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle_ch| chars.any(|text_ch| text_ch == needle_ch))
}
