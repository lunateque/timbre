//! The bounded parse of an `application/x-www-form-urlencoded` body: the
//! pair cap must stop the parse, not audit a list the body already built.

/// The pairs of an `application/x-www-form-urlencoded` body, in wire order,
/// materializing at most `budget` of them and reporting `more` when the body
/// held another pair beyond that. The bound is the point: a 1 MiB body of
/// `a&a&a...` carries over a hundred thousand pairs, and a pair bomb must
/// cost the budget's worth of parsing plus one sighted extra pair, never the
/// whole body materialized before the cap answers.
#[must_use]
pub(crate) fn form_pairs(body: &[u8], budget: usize) -> (Vec<(String, String)>, bool) {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for pair in form_urlencoded::parse(body).into_owned() {
        if pairs.len() >= budget {
            // This is pair `budget + 1`: sighted, never built into the list.
            return (pairs, true);
        }
        pairs.push(pair);
    }
    (pairs, false)
}

/// The merged query of a form POST: the URL pairs, then the form's in wire
/// order, materializing at most `query_limit + 1` pairs in total. A form
/// over the cap therefore always yields a list the parameter refusal logic
/// sees as over-cap (`params::refusal` decides `too_many_pairs` on its
/// length), and the pair bomb pays for the cap's worth of pairs plus one,
/// never for the whole body (see [`form_pairs`]).
#[must_use]
pub(crate) fn merge_form_pairs(
    url: Vec<(String, String)>,
    body: &[u8],
    query_limit: usize,
) -> Vec<(String, String)> {
    let budget = query_limit.saturating_sub(url.len()).saturating_add(1);
    let (pairs, _) = form_pairs(body, budget);
    let mut merged = url;
    merged.extend(pairs);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_pairs_keep_order_and_decode() {
        let body = b"q=%D0%BF%D0%BE%D0%B8%C1%81k&n=1&q=two";
        let (pairs, _) = form_pairs(body, 10);
        assert_eq!(pairs.len(), 3, "repeated keys survive");
        assert_eq!(pairs[2], ("q".to_owned(), "two".to_owned()));
    }

    #[test]
    fn a_form_parse_stops_at_the_budget_plus_one_sighted_pair() {
        let bomb = "a&a&".repeat(1000);
        let (pairs, more) = form_pairs(bomb.as_bytes(), 4);
        assert_eq!(pairs.len(), 4, "exactly the budget is materialized");
        assert!(more, "the extra pair is reported, not built");
        // Inside the budget nothing is reported and the whole list arrives.
        let (exact, more) = form_pairs(b"a&b&c", 3);
        assert_eq!(exact.len(), 3);
        assert!(!more);
    }

    #[test]
    fn a_merge_over_the_query_cap_leaves_a_refusable_list_without_the_rest() {
        // A URL already at the cap: the budget for the body is one pair, so
        // a 1 MiB `a&a&a...` body costs one materialized pair.
        let url = vec![("u".to_owned(), "1".to_owned()); 5];
        let bomb = "a&".repeat(100_000);
        let merged = merge_form_pairs(url, bomb.as_bytes(), 5);
        assert_eq!(merged.len(), 6, "the cap plus the one pair that crossed it");
        // A form that fits merges whole, URL pairs first, wire order kept.
        let url = vec![("a".to_owned(), "1".to_owned())];
        let merged = merge_form_pairs(url, b"b=2&a=9", 10);
        assert_eq!(
            merged,
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned()),
                ("a".to_owned(), "9".to_owned())
            ]
        );
    }
}
