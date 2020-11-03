use rust_icu_sys as sys;
use rust_icu_ubrk as brk;

pub fn tokenize(text: &str) -> impl Iterator<Item = &str> {
    let iter =
        brk::UBreakIterator::try_new(sys::UBreakIteratorType::UBRK_WORD, "en", text).unwrap();
    let mut ids = text.char_indices().skip(1);
    iter.scan((0, 0), move |s, x| {
        let (l, prev) = *s;
        let x = x as usize;
        if let Some((r, _)) = ids.nth(x - prev - 1) {
            *s = (r, x);
            Some(&text[l..r])
        } else {
            Some(&text[l..])
        }
    })
}

#[test]
fn test_tokenize() {
    let tokens: Vec<_> = tokenize("今日はいい天気だ").collect();
    assert_eq!(tokens, &["今日", "は", "いい", "天気", "だ"])
}
