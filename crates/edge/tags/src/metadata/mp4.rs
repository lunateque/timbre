use std::borrow::Cow;

use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};

pub(super) fn mp4_atom<'a>(tag: &'a Ilst, key: &str) -> Option<&'a Atom<'static>> {
    if let Some(name) = key.strip_prefix(':') {
        return freeform(tag, name);
    }
    let mut chars = key.chars();
    let ident = [0; 4].map(|_| chars.next().and_then(|ch| u8::try_from(u32::from(ch)).ok()));
    let [Some(a), Some(b), Some(c), Some(d)] = ident else {
        return None;
    };
    tag.get(&AtomIdent::Fourcc([a, b, c, d]))
}

pub(super) fn freeform<'a>(tag: &'a Ilst, name: &str) -> Option<&'a Atom<'static>> {
    tag.get(&AtomIdent::Freeform { mean: Cow::Borrowed("com.apple.iTunes"), name: Cow::Borrowed(name) })
        .or_else(|| tag.into_iter().find(|atom| matches!(atom.ident(), AtomIdent::Freeform { mean, name: candidate } if mean == "com.apple.iTunes" && candidate.eq_ignore_ascii_case(name))))
}

pub(super) fn mp4_number(tag: &Ilst, key: &str) -> Option<u32> {
    mp4_atom(tag, key)?.data().find_map(|data| match data {
        AtomData::SignedInteger(n) => u32::try_from(*n).ok(),
        AtomData::UnsignedInteger(n) => Some(*n),
        AtomData::Bool(flag) => Some(u32::from(*flag)),
        AtomData::Unknown { data, .. } => match data.as_slice() {
            [a] => Some(u32::from(*a)),
            [a, b] => Some(u32::from(u16::from_be_bytes([*a, *b]))),
            [a, b, c, d] => Some(u32::from_be_bytes([*a, *b, *c, *d])),
            _ => None,
        },
        _ => None,
    })
}

pub(super) fn mp4_pair(tag: &Ilst, key: &str) -> (Option<u32>, Option<u32>) {
    mp4_atom(tag, key)
        .and_then(|atom| {
            atom.data().find_map(|data| match data {
                AtomData::Unknown { data, .. } => {
                    let [_, _, a, b, c, d, ..] = data.as_slice() else {
                        return None;
                    };
                    Some((
                        Some(u32::from(u16::from_be_bytes([*a, *b]))),
                        Some(u32::from(u16::from_be_bytes([*c, *d]))),
                    ))
                }
                _ => None,
            })
        })
        .unwrap_or_default()
}
