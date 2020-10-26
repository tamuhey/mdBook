use rust_icu_sys as sys;
use rust_icu_ubrk as brk;

pub fn tokenize(text: &str) -> impl Iterator<Item = &str> {
    let iter =
        brk::UBreakIterator::try_new(sys::UBreakIteratorType::UBRK_WORD, "en", text).unwrap();
    let mut ids = text.char_indices().skip(1);
    let n = text.len();
    iter.scan((0, 0), move |s, x| {
        let (l, prev) = *s;
        let x = x as usize;
        let r = ids.nth(x - prev - 1).map(|(a, _)| a).unwrap_or(n);
        *s = (r, x);
        Some(&text[l..r])
    })
}
