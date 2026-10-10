//! Tidying a recognised transcript: what cloud recognition models are careless about.
//!
//! A model promises cues "up to 7 seconds and 90 characters" but on real footage returns 27 s and
//! 234 characters; the words it lists for a cue drift from the cue's text (the tail of the next
//! cue, or a missing end) although words and text agree in sequence. So the **text is the truth**:
//! every text word gets its time by matching against the model's word times over the whole track
//! (longest common subsequence of normalised letters), and words between matches share the gap by
//! length. A cue with at least half of its words matched is cut by words; otherwise by text, with
//! times proportional to the text. Ported from Jilo Studio (`shared/src/transcript.ts`).
//!
//! Times are seconds (`f64`) in one time base (the clip's).

/// A word with its time.
#[derive(Clone, Debug, PartialEq)]
pub struct CueWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

impl CueWord {
    pub fn new(text: impl Into<String>, start: f64, end: f64) -> Self {
        Self { text: text.into(), start, end }
    }
}

/// A recognised phrase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// The model's speaker label (`"S1"`), when it gave one.
    pub speaker: Option<String>,
    /// Word times the model gave (they may disagree with `text`).
    pub words: Option<Vec<CueWord>>,
}

/// Size limits of a cue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueLimits {
    /// Characters: two lines of subtitles.
    pub max_chars: usize,
    /// Seconds on screen.
    pub max_seconds: f64,
}

pub const CUE_LIMITS: CueLimits = CueLimits { max_chars: 84, max_seconds: 7.0 };

/// Words farther than this from their cue are another cue's: the model mixed up the time.
const WORD_DRIFT_SEC: f64 = 10.0;
/// Text words per matching block (memory grows with the square of a block).
const ALIGN_BLOCK: usize = 400;
/// Share of a cue's words that must match for its word times to be trusted.
const MIN_MATCHED: f64 = 0.5;

/// Short function words that must not hang at the end of a cue (they start the next phrase).
const HANGING: &[&str] = &[
    "и",
    "а",
    "но",
    "или",
    "что",
    "чтобы",
    "как",
    "в",
    "во",
    "на",
    "с",
    "со",
    "к",
    "ко",
    "по",
    "о",
    "об",
    "у",
    "из",
    "за",
    "для",
    "от",
    "до",
    "не",
    "ни",
    "же",
    "то",
    "это",
    "если",
    "когда",
    "который",
    "которые",
    "которая",
    "the",
    "a",
    "an",
    "and",
    "or",
    "but",
    "to",
    "of",
    "in",
    "on",
    "at",
    "for",
    "with",
    "that",
    "ва",
    "ки",
    "дар",
    "бо",
    "аз",
    "ба",
    "барои",
    "агар",
];

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Ends a phrase (`.`, `!`, `?`, `…`, optionally followed by a closing quote or bracket).
pub fn strong_break(text: &str) -> bool {
    ends_with(text, &['.', '!', '?', '…'])
}

/// A softer cut point (`,`, `;`, `:`, dashes).
pub fn soft_break(text: &str) -> bool {
    ends_with(text, &[',', ';', ':', '—', '–', '-'])
}

fn ends_with(text: &str, set: &[char]) -> bool {
    let mut it = text.chars().rev();
    match it.next() {
        Some('"' | '»' | ')') => it.next().is_some_and(|c| set.contains(&c)),
        Some(c) => set.contains(&c),
        None => false,
    }
}

/// A word for matching: letters and digits, lower case, "ё" as "е".
pub fn word_key(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).map(|c| if c == 'ё' { 'е' } else { c }).filter(|c| c.is_alphanumeric()).collect()
}

/// Words in time order, each at least 50 ms long, not overlapping.
pub fn clean_words(words: &[CueWord]) -> Vec<CueWord> {
    let mut sorted: Vec<CueWord> =
        words.iter().map(|w| CueWord::new(w.text.trim(), w.start, w.end)).filter(|w| !w.text.is_empty() && w.start.is_finite() && w.end.is_finite()).collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<CueWord> = Vec::with_capacity(sorted.len());
    for w in sorted {
        let start = out.last().map_or(w.start, |p| w.start.max(p.end));
        out.push(CueWord { end: w.end.max(start + 0.05), start, text: w.text });
    }
    out
}

struct Token {
    cue: usize,
    text: String,
    key: String,
    time: Option<(f64, f64)>,
}

/// Word times for the text of `cues`: text words are matched to the model's words (longest common
/// subsequence of letters) in blocks by time; matched words take the model's time, the others a
/// share of the gap between their matched neighbours by length. `None` for a cue where fewer than
/// half of the words matched.
pub fn align_cue_words(cues: &[Cue]) -> Vec<Option<Vec<CueWord>>> {
    let all: Vec<CueWord> = cues.iter().flat_map(|c| c.words.iter().flatten().cloned()).collect();
    let pool = clean_words(&all);
    let pool_keys: Vec<String> = pool.iter().map(|w| word_key(&w.text)).collect();
    let mut tokens: Vec<Token> = Vec::new();
    for (ci, cue) in cues.iter().enumerate() {
        for piece in cue.text.split_whitespace() {
            let key = word_key(piece);
            // a dash or another lone mark sticks to the previous word of the same cue
            if key.is_empty()
                && let Some(prev) = tokens.last_mut().filter(|t| t.cue == ci)
            {
                prev.text.push(' ');
                prev.text.push_str(piece);
                continue;
            }
            tokens.push(Token { cue: ci, text: piece.to_string(), key, time: None });
        }
    }
    let mut from = 0;
    while from < tokens.len() {
        let to = (from + ALIGN_BLOCK).min(tokens.len());
        let first = &cues[tokens[from].cue];
        let last = &cues[tokens[to - 1].cue];
        let (lo, hi) = (first.start - 5.0, last.end + 5.0);
        let idx: Vec<usize> = (0..pool.len()).filter(|&j| pool[j].end >= lo && pool[j].start <= hi).collect();
        let (n, m) = (to - from, idx.len());
        if m > 0 {
            let w = m + 1;
            let mut table = vec![0u16; (n + 1) * w];
            let same = |i: usize, j: usize, tokens: &[Token]| {
                let k = &tokens[from + i].key;
                !k.is_empty() && *k == pool_keys[idx[j]]
            };
            for i in (0..n).rev() {
                for j in (0..m).rev() {
                    table[i * w + j] = if same(i, j, &tokens) { table[(i + 1) * w + j + 1] + 1 } else { table[(i + 1) * w + j].max(table[i * w + j + 1]) };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < n && j < m {
                if same(i, j, &tokens) {
                    let p = &pool[idx[j]];
                    tokens[from + i].time = Some((p.start, p.end));
                    i += 1;
                    j += 1;
                } else if table[(i + 1) * w + j] >= table[i * w + j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        from = to;
    }
    cues.iter()
        .enumerate()
        .map(|(ci, cue)| {
            let own: Vec<&Token> = tokens.iter().filter(|t| t.cue == ci).collect();
            if own.is_empty() {
                return None;
            }
            let matched = own.iter().filter(|t| t.time.is_some()).count();
            if (matched as f64) / (own.len() as f64) < MIN_MATCHED {
                return None;
            }
            let mut out = Vec::with_capacity(own.len());
            let mut k = 0;
            while k < own.len() {
                if let Some((s, e)) = own[k].time {
                    out.push(CueWord::new(own[k].text.clone(), s, e));
                    k += 1;
                    continue;
                }
                let mut e = k;
                while e < own.len() && own[e].time.is_none() {
                    e += 1;
                }
                let next_start = own.get(e).and_then(|t| t.time).map(|t| t.0);
                let left = if k > 0 { own[k - 1].time.map_or(cue.start, |t| t.1) } else { cue.start.min(next_start.unwrap_or(cue.start)) };
                let right = next_start.unwrap_or_else(|| cue.end.max(left));
                let span = (right - left).max(0.0);
                let total: usize = own[k..e].iter().map(|t| chars(&t.text)).sum();
                let mut cursor = left;
                for t in &own[k..e] {
                    let d = if total > 0 { span * chars(&t.text) as f64 / total as f64 } else { 0.0 };
                    out.push(CueWord::new(t.text.clone(), cursor, cursor + d));
                    cursor += d;
                }
                k = e;
            }
            Some(clean_words(&out))
        })
        .collect()
}

fn cue_from_words(words: Vec<CueWord>, base: &Cue) -> Cue {
    Cue {
        start: words.first().map_or(base.start, |w| w.start),
        end: words.last().map_or(base.end, |w| w.end),
        text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
        speaker: base.speaker.clone(),
        words: Some(words),
    }
}

fn group_len(words: &[CueWord]) -> usize {
    words.iter().map(|w| chars(&w.text)).sum::<usize>() + words.len().saturating_sub(1)
}

fn ends_with_punct(text: &str) -> bool {
    text.chars().last().is_some_and(|c| ".,!?;:…".contains(c))
}

/// Cut words into cues within the limits: at the end of a phrase, else after a comma in the second
/// half, else before the word that no longer fits; a pause over 0.7 s is a boundary too.
fn split_words(words: &[CueWord], limits: CueLimits) -> Vec<Vec<CueWord>> {
    let mut groups: Vec<Vec<CueWord>> = Vec::new();
    let mut current: Vec<CueWord> = Vec::new();
    // hand over `count` words; function words hanging at the end move to the next cue
    let flush = |count: usize, current: &mut Vec<CueWord>, groups: &mut Vec<Vec<CueWord>>| {
        let mut cut = count;
        while cut > 1 && HANGING.contains(&word_key(&current[cut - 1].text).as_str()) && !ends_with_punct(&current[cut - 1].text) {
            cut -= 1;
        }
        let rest = current.split_off(cut);
        groups.push(std::mem::replace(current, rest));
    };
    for word in words {
        if let (Some(first), Some(last)) = (current.first(), current.last()) {
            let len = group_len(&current) + 1 + chars(&word.text);
            let too_long = len > limits.max_chars || word.end - first.start > limits.max_seconds;
            let pause = word.start - last.end > 0.7;
            if too_long || pause {
                let half = current.len() as f64 * 0.5;
                let soft = if pause { None } else { (0..current.len()).rev().find(|&i| i as f64 >= half && soft_break(&current[i].text)) };
                let n = match soft {
                    Some(s) if s + 1 < current.len() => s + 1,
                    _ => current.len(),
                };
                flush(n, &mut current, &mut groups);
            }
        }
        current.push(word.clone());
        if strong_break(&word.text) && group_len(&current) >= 15 {
            let n = current.len();
            flush(n, &mut current, &mut groups);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups.retain(|g| !g.is_empty());
    // a single word of a fraction of a second is not a cue: join a neighbour that has room
    let mut i = groups.len();
    while i > 0 {
        i -= 1;
        let g = &groups[i];
        if g.len() != 1 || g[0].end - g[0].start >= 0.4 || groups.len() == 1 {
            continue;
        }
        let one = groups[i].clone();
        if i > 0 && group_len(&groups[i - 1]) + 1 + group_len(&one) <= limits.max_chars {
            groups[i - 1].extend(one);
        } else if i + 1 < groups.len() && group_len(&one) + 1 + group_len(&groups[i + 1]) <= limits.max_chars {
            groups[i + 1].splice(0..0, one);
        } else {
            continue;
        }
        groups.remove(i);
    }
    groups
}

/// A cue without words, cut at punctuation (else at spaces) into parts within the limits; the time
/// is shared in proportion to the text.
fn split_text(cue: &Cue, limits: CueLimits) -> Vec<Cue> {
    let duration = cue.end - cue.start;
    let text = cue.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let parts = (chars(&text).div_ceil(limits.max_chars)).max((duration / limits.max_seconds).ceil() as usize).max(1);
    let base = Cue { text: text.clone(), words: None, ..cue.clone() };
    if parts <= 1 {
        return vec![base];
    }
    let target = chars(&text) as f64 / parts as f64;
    let mut pieces: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for token in text.split(' ') {
        if !current.is_empty() && chars(&current.join(" ")) + 1 + chars(token) > limits.max_chars {
            pieces.push(current.join(" "));
            current.clear();
        }
        current.push(token);
        let size = chars(&current.join(" ")) as f64;
        let left = parts as i64 - pieces.len() as i64 - 1;
        if left > 0 && (size >= target || (size >= target * 0.6 && (strong_break(token) || (size >= target * 0.8 && soft_break(token))))) {
            pieces.push(current.join(" "));
            current.clear();
        }
    }
    if !current.is_empty() {
        pieces.push(current.join(" "));
    }
    let total: usize = pieces.iter().map(|p| chars(p)).sum::<usize>().max(1);
    let mut cursor = cue.start;
    let last = pieces.len().saturating_sub(1);
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let end = if i == last { cue.end } else { cursor + duration * chars(&p) as f64 / total as f64 };
            let c = Cue { start: cursor, end, text: p, speaker: cue.speaker.clone(), words: None };
            cursor = end;
            c
        })
        .collect()
}

/// At a cue boundary the model repeats the end of the previous cue ("…with a dark background, /
/// with a dark background, however…"): a cue starting with the same two or more words the previous
/// one ended with loses the repeat, with its words.
fn drop_boundary_repeats(cues: Vec<Cue>) -> Vec<Cue> {
    let mut out: Vec<Cue> = Vec::with_capacity(cues.len());
    for cue in cues {
        let Some(prev) = out.last() else {
            out.push(cue);
            continue;
        };
        let before: Vec<&str> = prev.text.split_whitespace().collect();
        let after: Vec<&str> = cue.text.split_whitespace().collect();
        let keys = |w: &[&str]| w.iter().map(|x| word_key(x)).collect::<Vec<_>>().join(" ");
        let mut repeat = 0;
        let max = 8.min(before.len()).min(after.len().saturating_sub(1));
        for size in (2..=max).rev() {
            let tail = keys(&before[before.len() - size..]);
            if !tail.trim().is_empty() && tail == keys(&after[..size]) {
                repeat = size;
                break;
            }
        }
        if repeat == 0 {
            out.push(cue);
            continue;
        }
        let mut words = cue.words.clone();
        if let Some(ws) = words.as_mut() {
            for key in after[..repeat].iter().map(|x| word_key(x)) {
                if ws.first().is_some_and(|w| word_key(&w.text) == key) {
                    ws.remove(0);
                }
            }
        }
        out.push(Cue { text: after[repeat..].join(" "), words: words.filter(|w| !w.is_empty()), ..cue });
    }
    out
}

/// Put the cues of a recognised track in order: bounds from the words, long cues cut into parts,
/// overlaps resolved.
pub fn normalise_cues(cues: &[Cue], limits: CueLimits) -> Vec<Cue> {
    let mut sorted = cues.to_vec();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    let sorted = drop_boundary_repeats(sorted);
    let aligned = align_cue_words(&sorted);
    let mut out: Vec<Cue> = Vec::new();
    for (cue, words) in sorted.iter().zip(aligned) {
        let fits = words
            .as_ref()
            .is_some_and(|w| !w.is_empty() && (w[0].start - cue.start).abs() <= WORD_DRIFT_SEC && (w[w.len() - 1].end - cue.end).abs() <= WORD_DRIFT_SEC);
        match words.filter(|_| fits) {
            Some(w) => out.extend(split_words(&w, limits).into_iter().map(|g| cue_from_words(g, cue))),
            None => out.extend(split_text(cue, limits)),
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    for i in 1..out.len() {
        let (prev_start, prev_end) = (out[i - 1].start, out[i - 1].end);
        if out[i].start < prev_end {
            let middle = (prev_end + out[i].start) / 2.0;
            out[i - 1].end = (prev_start + 0.15).max(middle);
            let pe = out[i - 1].end;
            out[i].start = out[i].start.max(pe);
            if out[i].end < out[i].start + 0.15 {
                out[i].end = out[i].start + 0.15;
            }
        }
    }
    out
}

/// Sort, drop duplicates (the same text again within 1.5 s, from overlapping windows) and resolve
/// overlaps.
pub fn tidy(cues: &[Cue]) -> Vec<Cue> {
    let mut sorted = cues.to_vec();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<Cue> = Vec::with_capacity(sorted.len());
    for mut c in sorted {
        if let Some(prev) = out.last_mut() {
            if prev.text.trim() == c.text.trim() && c.start - prev.start < 1.5 {
                continue;
            }
            if c.start < prev.end {
                let middle = (prev.end + c.start) / 2.0;
                prev.end = (prev.start + 0.2).max(prev.end.min(middle));
                c.start = prev.end.max(c.start);
            }
        }
        if c.end - c.start < 0.15 {
            c.end = c.start + 0.15;
        }
        out.push(c);
    }
    out
}

/// Seconds of `speech` not covered by `spans` widened by `pad` on each side.
pub fn uncovered_speech(speech: &[(f64, f64)], spans: &[(f64, f64)], pad: f64) -> f64 {
    let mut covered: Vec<(f64, f64)> = spans.iter().map(|&(s, e)| (s - pad, e + pad)).collect();
    covered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    for &(start, end) in speech {
        let mut cursor = start;
        for &(cs, ce) in &covered {
            if ce <= cursor || cs >= end {
                continue;
            }
            if cs > cursor {
                total += cs - cursor;
            }
            cursor = cursor.max(ce);
            if cursor >= end {
                break;
            }
        }
        if cursor < end {
            total += end - cursor;
        }
    }
    total
}

/// Letters and digits in the texts (a check that tidying lost no text).
pub fn letter_count<'a>(texts: impl IntoIterator<Item = &'a str>) -> usize {
    texts.into_iter().map(|t| t.chars().filter(|c| c.is_alphanumeric()).count()).sum()
}

/// The words of tidied cues for a transcript: a cue's own words, or its text shared out over its
/// time by length. Each word carries its cue's speaker label.
pub fn cue_words(cues: &[Cue]) -> Vec<(CueWord, Option<String>)> {
    let mut out = Vec::new();
    for c in cues {
        match c.words.as_ref().filter(|w| !w.is_empty()) {
            Some(ws) => out.extend(ws.iter().cloned().map(|w| (w, c.speaker.clone()))),
            None => {
                let pieces: Vec<&str> = c.text.split_whitespace().collect();
                let total: usize = pieces.iter().map(|p| chars(p)).sum::<usize>().max(1);
                let mut cursor = c.start;
                for p in pieces {
                    let d = (c.end - c.start).max(0.0) * chars(p) as f64 / total as f64;
                    out.push((CueWord::new(p, cursor, cursor + d), c.speaker.clone()));
                    cursor += d;
                }
            }
        }
    }
    // the same cleaning as `clean_words`, keeping each word's speaker
    out.retain(|(w, _)| !w.text.trim().is_empty() && w.start.is_finite() && w.end.is_finite());
    out.sort_by(|a, b| a.0.start.total_cmp(&b.0.start));
    let mut prev_end = f64::NEG_INFINITY;
    for (w, _) in &mut out {
        w.text = w.text.trim().to_string();
        w.start = w.start.max(prev_end);
        w.end = w.end.max(w.start + 0.05);
        prev_end = w.end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, end: f64, text: &str, words: Option<Vec<(&str, f64, f64)>>) -> Cue {
        Cue { start, end, text: text.into(), speaker: None, words: words.map(|w| w.into_iter().map(|(t, s, e)| CueWord::new(t, s, e)).collect()) }
    }

    #[test]
    fn keys_ignore_case_punctuation_and_yo() {
        assert_eq!(word_key("Ёлка,"), "елка");
        assert_eq!(word_key("«Привет!»"), "привет");
        assert_eq!(word_key("—"), "");
        assert!(strong_break("конец.»") && strong_break("why?") && !strong_break("so,"));
        assert!(soft_break("однако,") && soft_break("так —") && !soft_break("слово"));
    }

    #[test]
    fn text_is_the_truth_and_unmatched_words_share_the_gap() {
        // the model's words lack "очень" and add "лишнее"
        let c = cue(0.0, 3.0, "Это очень хороший день.", Some(vec![("это", 0.0, 0.4), ("хороший", 1.0, 1.6), ("лишнее", 1.7, 1.9), ("день", 2.0, 2.5)]));
        let a = align_cue_words(&[c]);
        let w = a[0].as_ref().unwrap();
        assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Это", "очень", "хороший", "день."]);
        assert_eq!((w[0].start, w[0].end), (0.0, 0.4));
        // "очень" fills the gap between "это" and "хороший"
        assert!((w[1].start - 0.4).abs() < 1e-9 && (w[1].end - 1.0).abs() < 1e-9, "{:?}", w[1]);
        assert_eq!((w[3].start, w[3].end), (2.0, 2.5));
    }

    #[test]
    fn a_cue_matching_less_than_half_gets_no_words() {
        let c = cue(0.0, 2.0, "совсем другой текст тут", Some(vec![("hello", 0.0, 0.5), ("world", 0.5, 1.0)]));
        assert!(align_cue_words(&[c])[0].is_none());
    }

    #[test]
    fn long_cues_are_cut_at_phrase_ends_and_function_words_move_on() {
        let text = "Первая фраза здесь закончилась. А потом мы пошли гулять и увидели большой красивый старый дом на холме у реки";
        let words: Vec<(&str, f64, f64)> = text.split(' ').enumerate().map(|(i, w)| (w, i as f64 * 0.4, i as f64 * 0.4 + 0.35)).collect();
        let out = normalise_cues(&[cue(0.0, 8.0, text, Some(words))], CUE_LIMITS);
        assert!(out.len() >= 2, "{out:#?}");
        assert_eq!(out[0].text, "Первая фраза здесь закончилась.");
        for c in &out {
            assert!(chars(&c.text) <= CUE_LIMITS.max_chars && c.end - c.start <= CUE_LIMITS.max_seconds + 1e-9, "{c:?}");
            let last = c.text.split(' ').next_back().unwrap();
            assert!(!HANGING.contains(&word_key(last).as_str()) || c.text == out.last().unwrap().text, "hanging word in {c:?}");
        }
        // no text lost
        assert_eq!(letter_count(out.iter().map(|c| c.text.as_str())), letter_count([text]));
    }

    #[test]
    fn a_cue_without_words_is_cut_by_text_over_its_time() {
        let text = "Слово ".repeat(40);
        let out = normalise_cues(&[cue(10.0, 30.0, &text, None)], CUE_LIMITS);
        assert!(out.len() >= 3);
        assert!((out[0].start - 10.0).abs() < 1e-9 && (out.last().unwrap().end - 30.0).abs() < 1e-9);
        assert!(out.iter().all(|c| chars(&c.text) <= CUE_LIMITS.max_chars && c.words.is_none()));
        let words = cue_words(&out);
        assert_eq!(words.len(), 40);
        assert!(words.windows(2).all(|p| p[1].0.start >= p[0].0.end - 1e-9));
    }

    #[test]
    fn boundary_repeats_and_window_duplicates_are_dropped() {
        let a = cue(0.0, 2.0, "мы видим тёмным фоном,", None);
        let b = cue(2.0, 4.0, "тёмным фоном, однако всё хорошо", None);
        let out = normalise_cues(&[a, b], CUE_LIMITS);
        assert_eq!(out[1].text, "однако всё хорошо");
        let t = tidy(&[cue(5.0, 6.0, "Привет", None), cue(5.5, 6.5, "Привет", None), cue(5.8, 7.0, "Мир", None)]);
        assert_eq!(t.len(), 2);
        assert!(t[1].start >= t[0].end);
    }

    #[test]
    fn uncovered_speech_counts_gaps() {
        let speech = [(0.0, 10.0)];
        assert!((uncovered_speech(&speech, &[(0.0, 4.0), (6.0, 10.0)], 0.6) - 0.8).abs() < 1e-9);
        assert_eq!(uncovered_speech(&speech, &[(0.0, 10.0)], 0.0), 0.0);
        assert!((uncovered_speech(&speech, &[], 0.6) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn hostile_input_never_panics() {
        let weird = [cue(f64::NAN, 1.0, "", Some(vec![("", f64::NAN, f64::INFINITY)])), cue(5.0, 1.0, "— – -", None), cue(0.0, 0.0, "a", Some(vec![]))];
        let _ = normalise_cues(&weird, CUE_LIMITS);
        let _ = tidy(&weird);
        let _ = cue_words(&weird);
        assert!(normalise_cues(&[], CUE_LIMITS).is_empty());
    }
}
