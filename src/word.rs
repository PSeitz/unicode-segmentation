// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use core::cmp;

extern crate alloc;
use alloc::boxed::Box;

use crate::tables::word::WordCat;

/// External iterator for a string's
/// [word boundaries](http://www.unicode.org/reports/tr29/#Word_Boundaries).
///
/// This struct is created by the [`split_word_bounds`] method on the [`UnicodeSegmentation`]
/// trait. See its documentation for more.
///
/// [`split_word_bounds`]: trait.UnicodeSegmentation.html#tymethod.split_word_bounds
/// [`UnicodeSegmentation`]: trait.UnicodeSegmentation.html
#[derive(Debug, Clone)]
pub struct UWordBounds<'a> {
    string: &'a str,
    cat: Option<WordCat>,
    catb: Option<WordCat>,
}

/// External iterator for word boundaries and byte offsets.
///
/// This struct is created by the [`split_word_bound_indices`] method on the
/// [`UnicodeSegmentation`] trait. See its documentation for more.
///
/// [`split_word_bound_indices`]: trait.UnicodeSegmentation.html#tymethod.split_word_bound_indices
/// [`UnicodeSegmentation`]: trait.UnicodeSegmentation.html
#[derive(Debug, Clone)]
pub struct UWordBoundIndices<'a> {
    start_offset: usize,
    iter: UWordBounds<'a>,
}

impl<'a> Iterator for UWordBoundIndices<'a> {
    type Item = (usize, &'a str);

    #[inline]
    fn next(&mut self) -> Option<(usize, &'a str)> {
        self.iter
            .next()
            .map(|s| (s.as_ptr() as usize - self.start_offset, s))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> DoubleEndedIterator for UWordBoundIndices<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<(usize, &'a str)> {
        self.iter
            .next_back()
            .map(|s| (s.as_ptr() as usize - self.start_offset, s))
    }
}

// state machine for word boundary rules
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UWordBoundsState {
    Start,
    Letter,
    HLetter,
    Numeric,
    Katakana,
    ExtendNumLet,
    Regional(RegionalState),
    FormatExtend(FormatExtendType),
    Zwj,
    Emoji,
    WSegSpace,
}

// subtypes for FormatExtend state in UWordBoundsState
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FormatExtendType {
    AcceptAny,
    AcceptNone,
    RequireLetter,
    RequireHLetter,
    AcceptQLetter,
    RequireNumeric,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RegionalState {
    Half,
    Full,
    Unknown,
}

fn is_emoji(ch: char) -> bool {
    use crate::tables::emoji;
    emoji::emoji_category(ch).2 == emoji::EmojiCat::EC_Extended_Pictographic
}

impl<'a> Iterator for UWordBounds<'a> {
    type Item = &'a str;

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let slen = self.string.len();
        (cmp::min(slen, 1), Some(slen))
    }

    #[inline]
    fn next(&mut self) -> Option<&'a str> {
        use self::FormatExtendType::*;
        use self::UWordBoundsState::*;
        use crate::tables::word as wd;
        if self.string.is_empty() {
            return None;
        }

        let mut take_curr = true;
        let mut take_cat = true;
        let mut idx = 0;
        let mut saveidx = 0;
        let mut state = Start;
        let mut cat = wd::WC_Any;
        let mut savecat = wd::WC_Any;

        // If extend/format/zwj were skipped. Handles precedence of WB3d over WB4
        let mut skipped_format_extend = false;
        for (curr, ch) in self.string.char_indices() {
            idx = curr;
            // Whether or not the previous category was ZWJ
            // ZWJs get collapsed, so this handles precedence of WB3c over WB4
            let prev_zwj = cat == wd::WC_ZWJ;
            // if there's a category cached, grab it
            cat = match self.cat {
                None => wd::word_category(ch).2,
                _ => self.cat.take().unwrap(),
            };
            take_cat = true;

            // handle rule WB4
            // just skip all format, extend, and zwj chars
            // note that Start is a special case: if there's a bunch of Format | Extend
            // characters at the beginning of a block of text, dump them out as one unit.
            //
            // (This is not obvious from the wording of UAX#29, but if you look at the
            // test cases http://www.unicode.org/Public/UNIDATA/auxiliary/WordBreakTest.txt
            // then the "correct" interpretation of WB4 becomes apparent.)
            if state != Start {
                match cat {
                    wd::WC_Extend | wd::WC_Format | wd::WC_ZWJ => {
                        skipped_format_extend = true;
                        continue;
                    }
                    _ => {}
                }
            }

            // rule WB3c
            // WB4 makes all ZWJs collapse into the previous state
            // but you can still be in a Zwj state if you started with Zwj
            //
            // This means that an EP + Zwj will collapse into EP, which is wrong,
            // since EP+EP is not a boundary but EP+ZWJ+EP is
            //
            // Thus, we separately keep track of whether or not the last character
            // was a ZWJ. This is an additional bit of state tracked outside of the
            // state enum; the state enum represents the last non-zwj state encountered.
            // When prev_zwj is true, for the purposes of WB3c, we are in the Zwj state,
            // however we are in the previous state for the purposes of all other rules.
            if prev_zwj && is_emoji(ch) {
                state = Emoji;
                continue;
            }
            // Don't use `continue` in this match without updating `cat`
            state = match state {
                Start if cat == wd::WC_CR => {
                    idx += match self.get_next_cat(idx) {
                        Some(wd::WC_LF) => 1, // rule WB3
                        _ => 0,
                    };
                    break; // rule WB3a
                }
                Start => match cat {
                    wd::WC_ALetter => Letter,            // rule WB5, WB6, WB9, WB13a
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB5, WB6, WB7a, WB7b, WB9, WB13a
                    wd::WC_Numeric => Numeric,           // rule WB8, WB10, WB12, WB13a
                    wd::WC_Katakana => Katakana,         // rule WB13, WB13a
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a, WB13b
                    wd::WC_Regional_Indicator => Regional(RegionalState::Half), // rule WB13c
                    wd::WC_LF | wd::WC_Newline => break, // rule WB3a
                    wd::WC_ZWJ => Zwj,                   // rule WB3c
                    wd::WC_WSegSpace => WSegSpace,       // rule WB3d
                    _ => {
                        if let Some(ncat) = self.get_next_cat(idx) {
                            // rule WB4
                            if ncat == wd::WC_Format || ncat == wd::WC_Extend || ncat == wd::WC_ZWJ
                            {
                                state = FormatExtend(AcceptNone);
                                self.cat = Some(ncat);
                                continue;
                            }
                        }
                        break; // rule WB999
                    }
                },
                WSegSpace => match cat {
                    wd::WC_WSegSpace if !skipped_format_extend => WSegSpace,
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Zwj => {
                    // We already handle WB3c above.
                    take_curr = false;
                    break;
                }
                Letter | HLetter => match cat {
                    wd::WC_ALetter => Letter,            // rule WB5
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB5
                    wd::WC_Numeric => Numeric,           // rule WB9
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a
                    wd::WC_Double_Quote if state == HLetter => {
                        savecat = cat;
                        saveidx = idx;
                        FormatExtend(RequireHLetter) // rule WB7b
                    }
                    wd::WC_Single_Quote if state == HLetter => {
                        FormatExtend(AcceptQLetter) // rule WB7a
                    }
                    wd::WC_MidLetter | wd::WC_MidNumLet | wd::WC_Single_Quote => {
                        savecat = cat;
                        saveidx = idx;
                        FormatExtend(RequireLetter) // rule WB6
                    }
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Numeric => match cat {
                    wd::WC_Numeric => Numeric,           // rule WB8
                    wd::WC_ALetter => Letter,            // rule WB10
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB10
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a
                    wd::WC_MidNum | wd::WC_MidNumLet | wd::WC_Single_Quote => {
                        savecat = cat;
                        saveidx = idx;
                        FormatExtend(RequireNumeric) // rule WB12
                    }
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Katakana => match cat {
                    wd::WC_Katakana => Katakana,         // rule WB13
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                ExtendNumLet => match cat {
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a
                    wd::WC_ALetter => Letter,            // rule WB13b
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB13b
                    wd::WC_Numeric => Numeric,           // rule WB13b
                    wd::WC_Katakana => Katakana,         // rule WB13b
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Regional(RegionalState::Full) => {
                    // if it reaches here we've gone too far,
                    // a full flag can only compose with ZWJ/Extend/Format
                    // proceeding it.
                    take_curr = false;
                    break;
                }
                Regional(RegionalState::Half) => match cat {
                    wd::WC_Regional_Indicator => Regional(RegionalState::Full), // rule WB13c
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Regional(_) => {
                    unreachable!("RegionalState::Unknown should not occur on forward iteration")
                }
                Emoji => {
                    // We already handle WB3c above. If you've reached this point, the emoji sequence is over.
                    take_curr = false;
                    break;
                }
                FormatExtend(t) => match t {
                    // handle FormatExtends depending on what type
                    RequireNumeric if cat == wd::WC_Numeric => Numeric, // rule WB11
                    RequireLetter | AcceptQLetter if cat == wd::WC_ALetter => Letter, // rule WB7
                    RequireLetter | AcceptQLetter if cat == wd::WC_Hebrew_Letter => HLetter, // WB7a
                    RequireHLetter if cat == wd::WC_Hebrew_Letter => HLetter, // rule WB7b
                    AcceptNone | AcceptQLetter => {
                        take_curr = false; // emit all the Format|Extend characters
                        take_cat = false;
                        break;
                    }
                    _ => break, // rewind (in if statement below)
                },
            }
        }

        if let FormatExtend(t) = state {
            // we were looking for something and didn't find it; we have to back up
            if t == RequireLetter || t == RequireHLetter || t == RequireNumeric {
                idx = saveidx;
                cat = savecat;
                take_curr = false;
            }
        }

        self.cat = if take_curr {
            idx = idx + self.string[idx..].chars().next().unwrap().len_utf8();
            None
        } else if take_cat {
            Some(cat)
        } else {
            None
        };

        let retstr = &self.string[..idx];
        self.string = &self.string[idx..];
        Some(retstr)
    }
}

impl<'a> DoubleEndedIterator for UWordBounds<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a str> {
        use self::FormatExtendType::*;
        use self::UWordBoundsState::*;
        use crate::tables::word as wd;
        if self.string.is_empty() {
            return None;
        }

        let mut take_curr = true;
        let mut take_cat = true;
        let mut idx = self.string.len();
        idx -= self.string.chars().next_back().unwrap().len_utf8();
        let mut previdx = idx;
        let mut saveidx = idx;
        let mut state = Start;
        let mut savestate = Start;
        let mut cat = wd::WC_Any;

        let mut skipped_format_extend = false;

        for (curr, ch) in self.string.char_indices().rev() {
            previdx = idx;
            idx = curr;

            // if there's a category cached, grab it
            cat = match self.catb {
                None => wd::word_category(ch).2,
                _ => self.catb.take().unwrap(),
            };
            take_cat = true;

            // backward iterator over word boundaries. Mostly the same as the forward
            // iterator, with two weirdnesses:
            // (1) If we encounter a single quote in the Start state, we have to check for a
            //     Hebrew Letter immediately before it.
            // (2) Format and Extend char handling takes some gymnastics.

            if cat == wd::WC_Extend || cat == wd::WC_Format || (cat == wd::WC_ZWJ && state != Zwj) {
                // WB3c has more priority so we should not
                // fold in that case
                if !matches!(state, FormatExtend(_) | Start) {
                    saveidx = previdx;
                    savestate = state;
                    state = FormatExtend(AcceptNone);
                }

                if state != Start {
                    continue;
                }
            } else if state == FormatExtend(AcceptNone) {
                // finished a scan of some Format|Extend chars, restore previous state
                state = savestate;
                previdx = saveidx;
                take_cat = false;
                skipped_format_extend = true;
            }

            // Don't use `continue` in this match without updating `catb`
            state = match state {
                Start | FormatExtend(AcceptAny) => match cat {
                    _ if is_emoji(ch) => Zwj,
                    wd::WC_ALetter => Letter, // rule WB5, WB7, WB10, WB13b
                    wd::WC_Hebrew_Letter => HLetter, // rule WB5, WB7, WB7c, WB10, WB13b
                    wd::WC_Numeric => Numeric, // rule WB8, WB9, WB11, WB13b
                    wd::WC_Katakana => Katakana, // rule WB13, WB13b
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a
                    wd::WC_Regional_Indicator => Regional(RegionalState::Unknown), // rule WB13c
                    // rule WB4:
                    wd::WC_Extend | wd::WC_Format | wd::WC_ZWJ => FormatExtend(AcceptAny),
                    wd::WC_Single_Quote => {
                        saveidx = idx;
                        FormatExtend(AcceptQLetter) // rule WB7a
                    }
                    wd::WC_WSegSpace => WSegSpace,
                    wd::WC_CR | wd::WC_LF | wd::WC_Newline => {
                        if state == Start {
                            if cat == wd::WC_LF {
                                idx -= match self.get_prev_cat(idx) {
                                    Some(wd::WC_CR) => 1, // rule WB3
                                    _ => 0,
                                };
                            }
                        } else {
                            take_curr = false;
                        }
                        break; // rule WB3a
                    }
                    _ => break, // rule WB999
                },
                Zwj => match cat {
                    // rule WB3c
                    wd::WC_ZWJ => FormatExtend(AcceptAny),
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                WSegSpace => match cat {
                    // rule WB3d
                    wd::WC_WSegSpace if !skipped_format_extend => WSegSpace,
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Letter | HLetter => match cat {
                    wd::WC_ALetter => Letter,            // rule WB5
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB5
                    wd::WC_Numeric => Numeric,           // rule WB10
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13b
                    wd::WC_Double_Quote if state == HLetter => {
                        saveidx = previdx;
                        FormatExtend(RequireHLetter) // rule WB7c
                    }
                    wd::WC_MidLetter | wd::WC_MidNumLet | wd::WC_Single_Quote => {
                        saveidx = previdx;
                        FormatExtend(RequireLetter) // rule WB7
                    }
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Numeric => match cat {
                    wd::WC_Numeric => Numeric,           // rule WB8
                    wd::WC_ALetter => Letter,            // rule WB9
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB9
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13b
                    wd::WC_MidNum | wd::WC_MidNumLet | wd::WC_Single_Quote => {
                        saveidx = previdx;
                        FormatExtend(RequireNumeric) // rule WB11
                    }
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Katakana => match cat {
                    wd::WC_Katakana => Katakana,         // rule WB13
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13b
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                ExtendNumLet => match cat {
                    wd::WC_ExtendNumLet => ExtendNumLet, // rule WB13a
                    wd::WC_ALetter => Letter,            // rule WB13a
                    wd::WC_Hebrew_Letter => HLetter,     // rule WB13a
                    wd::WC_Numeric => Numeric,           // rule WB13a
                    wd::WC_Katakana => Katakana,         // rule WB13a
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Regional(mut regional_state) => match cat {
                    // rule WB13c
                    wd::WC_Regional_Indicator => {
                        if regional_state == RegionalState::Unknown {
                            let count = self.string[..previdx]
                                .chars()
                                .rev()
                                .map(|c| wd::word_category(c).2)
                                .filter(|&c| {
                                    !(c == wd::WC_ZWJ || c == wd::WC_Extend || c == wd::WC_Format)
                                })
                                .take_while(|&c| c == wd::WC_Regional_Indicator)
                                .count();
                            regional_state = if count % 2 == 0 {
                                RegionalState::Full
                            } else {
                                RegionalState::Half
                            };
                        }
                        if regional_state == RegionalState::Full {
                            take_curr = false;
                            break;
                        } else {
                            Regional(RegionalState::Full)
                        }
                    }
                    _ => {
                        take_curr = false;
                        break;
                    }
                },
                Emoji => {
                    if is_emoji(ch) {
                        // rule WB3c
                        Zwj
                    } else {
                        take_curr = false;
                        break;
                    }
                }
                FormatExtend(t) => match t {
                    RequireNumeric if cat == wd::WC_Numeric => Numeric, // rule WB12
                    RequireLetter if cat == wd::WC_ALetter => Letter,   // rule WB6
                    RequireLetter if cat == wd::WC_Hebrew_Letter => HLetter, // rule WB6
                    AcceptQLetter if cat == wd::WC_Hebrew_Letter => HLetter, // rule WB7a
                    RequireHLetter if cat == wd::WC_Hebrew_Letter => HLetter, // rule WB7b
                    _ => break,                                         // backtrack will happens
                },
            }
        }

        if let FormatExtend(t) = state {
            // if we required something but didn't find it, backtrack
            if t == RequireLetter
                || t == RequireHLetter
                || t == RequireNumeric
                || t == AcceptNone
                || t == AcceptQLetter
            {
                previdx = saveidx;
                take_cat = false;
                take_curr = false;
            }
        }

        self.catb = if take_curr {
            None
        } else {
            idx = previdx;
            if take_cat {
                Some(cat)
            } else {
                None
            }
        };

        let retstr = &self.string[idx..];
        self.string = &self.string[..idx];
        Some(retstr)
    }
}

impl<'a> UWordBounds<'a> {
    #[inline]
    fn get_next_cat(&self, idx: usize) -> Option<WordCat> {
        use crate::tables::word as wd;
        let nidx = idx + self.string[idx..].chars().next().unwrap().len_utf8();
        if nidx < self.string.len() {
            let nch = self.string[nidx..].chars().next().unwrap();
            Some(wd::word_category(nch).2)
        } else {
            None
        }
    }

    #[inline]
    fn get_prev_cat(&self, idx: usize) -> Option<WordCat> {
        use crate::tables::word as wd;
        if idx > 0 {
            let nch = self.string[..idx].chars().next_back().unwrap();
            Some(wd::word_category(nch).2)
        } else {
            None
        }
    }
}

#[inline]
pub fn new_word_bounds(s: &str) -> UWordBounds<'_> {
    UWordBounds {
        string: s,
        cat: None,
        catb: None,
    }
}

#[inline]
pub fn new_word_bound_indices(s: &str) -> UWordBoundIndices<'_> {
    UWordBoundIndices {
        start_offset: s.as_ptr() as usize,
        iter: new_word_bounds(s),
    }
}

#[inline]
fn has_alphanumeric(s: &&str) -> bool {
    s.chars().any(|c| c.is_alphanumeric())
}

#[inline]
fn has_ascii_alphanumeric(s: &&str) -> bool {
    s.bytes().any(|b| b.is_ascii_alphanumeric())
}

/// Fast-path for ASCII-only word segmentation, matching `unicode-segmentation` on pure ASCII:
/// • runs of ASCII spaces are grouped (`"   "`)  
/// • core-runs (letters, digits, underscore + infix)  
/// • any other ASCII char emits as one token, except CR+LF emits as a single two-char token
pub fn new_ascii_word_bound_indices<'a>(s: &'a str) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    #[inline(always)]
    fn is_core(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }
    #[inline(always)]
    fn is_infix(b: u8, prev: u8, next: u8) -> bool {
        match b {
            // numeric separators
            b'.' | b',' | b';' | b'\'' if prev.is_ascii_digit() && next.is_ascii_digit() => true,
            // apostrophe in contractions
            b'\'' if prev.is_ascii_alphabetic() && next.is_ascii_alphabetic() => true,
            // dot/colon inside letters
            b'.' | b':' if prev.is_ascii_alphabetic() && next.is_ascii_alphabetic() => true,
            _ => false,
        }
    }

    use core::iter::from_fn;
    let mut rest = s;
    let mut offset = 0;

    from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let bytes = rest.as_bytes();
        let len = bytes.len();

        // 1) Group runs of spaces
        if bytes[0] == b' ' {
            let mut i = 1;
            while i < len && bytes[i] == b' ' {
                i += 1;
            }
            let word = &rest[..i];
            let pos = offset;
            rest = &rest[i..];
            offset += i;
            return Some((pos, word));
        }

        // 2) Core-run (letters/digits/underscore + infix)
        if is_core(bytes[0]) {
            let mut i = 1;
            while i < len {
                let b = bytes[i];
                if is_core(b) || (i + 1 < len && is_infix(b, bytes[i - 1], bytes[i + 1])) {
                    i += 1;
                } else {
                    break;
                }
            }
            let word = &rest[..i];
            let pos = offset;
            rest = &rest[i..];
            offset += i;
            return Some((pos, word));
        }

        // 3) Non-core: CR+LF as one token, otherwise single char
        if bytes[0] == b'\r' && len >= 2 && bytes[1] == b'\n' {
            let word = &rest[..2];
            let pos = offset;
            rest = &rest[2..];
            offset += 2;
            Some((pos, word))
        } else {
            // emit exactly one byte (whitespace/control/punct)
            let word = &rest[..1];
            let pos = offset;
            rest = &rest[1..];
            offset += 1;
            Some((pos, word))
        }
    })
}
/// An iterator over the substrings of a string which, after splitting the string on
/// [word boundaries](http://www.unicode.org/reports/tr29/#Word_Boundaries),
/// contain any characters with the
/// [Alphabetic](http://unicode.org/reports/tr44/#Alphabetic)
/// property, or with
/// [General_Category=Number](http://unicode.org/reports/tr44/#General_Category_Values).
///
/// This method is accessed by the [`unicode_words`] method on the [`UnicodeSegmentation`] trait. See
/// its documentation for more.
///
/// [`unicode_words`]: trait.UnicodeSegmentation.html#tymethod.unicode_words
/// [`UnicodeSegmentation`]: trait.UnicodeSegmentation.html
#[inline]
pub(crate) fn new_unicode_words(s: &str) -> Box<dyn Iterator<Item = &str> + '_> {
    if s.is_ascii() {
        Box::new(new_unicode_words_ascii(s))
    } else {
        Box::new(new_unicode_words_general(s))
    }
}

#[inline]
fn new_unicode_words_ascii<'a>(s: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    new_ascii_word_bound_indices(s)
        .map(|(_, w)| w)
        .filter(|w| w.chars().any(|c| c.is_ascii_alphanumeric()))
}

#[inline]
fn new_unicode_words_general<'a>(s: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    new_word_bounds(s).filter(has_alphanumeric)
}

/// An iterator over the substrings of a string which, after splitting the string on
/// [word boundaries](http://www.unicode.org/reports/tr29/#Word_Boundaries),
/// contain any characters with the
/// [Alphabetic](http://unicode.org/reports/tr44/#Alphabetic)
/// property, or with
/// [General_Category=Number](http://unicode.org/reports/tr44/#General_Category_Values).
/// This iterator also provides the byte offsets for each substring.
///
/// This method is accessed by the [`unicode_word_indices`] method on the [`UnicodeSegmentation`] trait. See
/// its documentation for more.
///
/// [`unicode_word_indices`]: trait.UnicodeSegmentation.html#tymethod.unicode_word_indices
/// [`UnicodeSegmentation`]: trait.UnicodeSegmentation.html
#[inline]
pub fn new_unicode_word_indices<'a>(s: &'a str) -> Box<dyn Iterator<Item = (usize, &'a str)> + 'a> {
    if s.is_ascii() {
        Box::new(new_ascii_word_bound_indices(s).filter(|(_, w)| has_ascii_alphanumeric(w)))
    } else {
        Box::new(new_word_bound_indices(s).filter(|(_, w)| has_alphanumeric(w)))
    }
}

#[cfg(test)]
mod tests {
    use crate::word::{
        new_ascii_word_bound_indices, new_unicode_words_ascii, new_word_bound_indices,
    };
    use std::string::String;
    use std::vec::Vec;
    use std::{format, vec};

    use proptest::prelude::*;

    #[test]
    fn test_syriac_abbr_mark() {
        use crate::tables::word as wd;
        let (_, _, cat) = wd::word_category('\u{70f}');
        assert_eq!(cat, wd::WC_ALetter);
    }

    #[test]
    fn test_end_of_ayah_cat() {
        use crate::tables::word as wd;
        let (_, _, cat) = wd::word_category('\u{6dd}');
        assert_eq!(cat, wd::WC_Numeric);
    }

    #[test]
    fn test_ascii_word_indices_various_cases() {
        let s = "Hello, world! can't e.g. var1 123,456 foo_bar example.com 127.0.0.1:9090";
        let words: Vec<&str> = new_unicode_words_ascii(s).collect();
        let expected = vec![
            ("Hello"), // simple letters
            ("world"), // skip comma+space, stop at '!'
            ("can't"), // apostrophe joins letters
            ("e.g"),
            ("var1"),
            ("123,456"), // digits+comma+digits
            ("foo_bar"),
            ("example.com"),
            ("127.0.0.1"),
            ("9090"), // port number
        ];
        assert_eq!(words, expected);
    }

    /// Strategy that yields every code-point from NUL (0) to DEL (127).
    fn ascii_char() -> impl Strategy<Value = char> {
        (0u8..=127).prop_map(|b| b as char)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10000))]
        /// Fast path must equal general path for any ASCII input.
        #[test]
        fn proptest_ascii_matches_unicode_word_indices(
            // Vec<char> → String, length 0‒99
            s in proptest::collection::vec(ascii_char(), 0..100)
                   .prop_map(|v| v.into_iter().collect::<String>())
        ) {
            let fast: Vec<(usize, &str)> = new_ascii_word_bound_indices(&s).collect();
            let uni:  Vec<(usize, &str)> = new_word_bound_indices(&s).collect();

            prop_assert_eq!(fast, uni);
        }
    }
}
