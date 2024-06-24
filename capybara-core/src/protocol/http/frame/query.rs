use std::borrow::Cow;

use memchr::memmem;
use smallvec::SmallVec;

#[derive(Debug)]
pub struct Queries<'a>(&'a [u8]);

impl Queries<'_> {
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            queries: self,
            cur: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0
    }

    pub fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.0) }
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    pub fn get_last<K>(&self, key: K) -> Option<&[u8]>
    where
        K: AsRef<[u8]>,
    {
        let key = key.as_ref();

        let mut result = None;

        if key.is_empty() {
            return result;
        }

        let finder = memmem::Finder::new(key);
        for i in finder.find_iter(self.0) {
            if i != 0 && self.0[i - 1] != b'&' {
                continue;
            }

            let mut offset = i + key.len();

            // EOF
            if offset == self.0.len() {
                result.replace(&b""[..]);
                break;
            }

            // check next char:
            //  - ...&foo&... => None
            //  - ...&foo=&... => None
            //  - ...&foo=xxx&... => Some(xxx)
            match self.0[offset] {
                b'&' => {
                    result.replace(&b""[..]);
                }
                b'=' => {
                    offset += 1;
                    let mut end = None;
                    for i in offset..self.0.len() {
                        if self.0[i] == b'&' {
                            end.replace(i);
                            break;
                        }
                    }
                    match end {
                        None => {
                            result.replace(&self.0[offset..]);
                        }
                        Some(j) => {
                            result.replace(&self.0[offset..j]);
                        }
                    }
                }
                _ => (),
            }
        }

        result
    }

    pub fn get_first<K>(&self, key: K) -> Option<&[u8]>
    where
        K: AsRef<[u8]>,
    {
        let key = key.as_ref();

        if key.is_empty() {
            return None;
        }

        let finder = memmem::Finder::new(key);
        for i in finder.find_iter(self.0) {
            if i != 0 && self.0[i - 1] != b'&' {
                continue;
            }

            let mut offset = i + key.len();

            // EOF
            if offset == self.0.len() {
                return Some(&b""[..]);
            }

            // check next char:
            //  - ...&foo&... => None
            //  - ...&foo=&... => None
            //  - ...&foo=xxx&... => Some(xxx)
            match self.0[offset] {
                b'&' => {
                    return Some(&b""[..]);
                }
                b'=' => {
                    offset += 1;
                    let mut end = None;
                    for i in offset..self.0.len() {
                        if self.0[i] == b'&' {
                            end.replace(i);
                            break;
                        }
                    }
                    return match end {
                        None => Some(&self.0[offset..]),
                        Some(j) => Some(&self.0[offset..j]),
                    };
                }
                _ => (),
            }
        }

        None
    }

    pub fn get_all<K>(&self, key: K) -> SmallVec<[&[u8]; 4]>
    where
        K: AsRef<[u8]>,
    {
        let key = key.as_ref();

        let mut result = SmallVec::<[&[u8]; 4]>::new();

        if key.is_empty() {
            return result;
        }

        let finder = memmem::Finder::new(key);
        for i in finder.find_iter(self.0) {
            if i != 0 && self.0[i - 1] != b'&' {
                continue;
            }

            let mut offset = i + key.len();

            // EOF
            if offset == self.0.len() {
                result.push(&b""[..]);
                break;
            }

            // check next char:
            //  - ...&foo&... => None
            //  - ...&foo=&... => None
            //  - ...&foo=xxx&... => Some(xxx)
            match self.0[offset] {
                b'&' => {
                    result.push(&b""[..]);
                }
                b'=' => {
                    offset += 1;
                    let mut end = None;
                    for i in offset..self.0.len() {
                        if self.0[i] == b'&' {
                            end.replace(i);
                            break;
                        }
                    }
                    match end {
                        None => result.push(&self.0[offset..]),
                        Some(j) => result.push(&self.0[offset..j]),
                    }
                }
                _ => (),
            }
        }

        result
    }

    pub fn get(&self, key: &str) -> Option<Query> {
        if key.is_empty() {
            return None;
        }

        let first = key.as_bytes().first().unwrap();
        for next in self.iter() {
            if next.0.is_empty() {
                continue;
            }

            if !next.0.first().unwrap().eq_ignore_ascii_case(first) {
                continue;
            }

            if next.key().eq_ignore_ascii_case(key) {
                return Some(next);
            }
        }

        None
    }
}

impl<'a> From<&'a [u8]> for Queries<'a> {
    fn from(value: &'a [u8]) -> Self {
        Self(value)
    }
}

#[derive(Debug)]
pub struct Query<'a>(pub(crate) &'a [u8]);

impl Query<'_> {
    pub fn key(&self) -> Cow<str> {
        match self.pos() {
            Some(i) => String::from_utf8_lossy(&self.0[..i]),
            None => String::from_utf8_lossy(self.0),
        }
    }

    pub fn value_bytes(&self) -> Option<&[u8]> {
        self.pos().map(|i| &self.0[i + 1..])
    }

    pub fn value(&self) -> Option<Cow<str>> {
        self.pos()
            .map(|i| String::from_utf8_lossy(&self.0[i + 1..]))
    }

    #[inline]
    fn pos(&self) -> Option<usize> {
        self.0.iter().position(|b| *b == b'=')
    }

    pub fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.0) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0
    }
}

impl<'a> AsRef<str> for Query<'a> {
    fn as_ref(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.0) }
    }
}

impl<'a> AsRef<[u8]> for Query<'a> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for Query<'a> {
    fn from(b: &'a [u8]) -> Self {
        Self(b)
    }
}

pub struct Iter<'a> {
    queries: &'a Queries<'a>,
    cur: isize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Query<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // eof
        if self.cur == -1 {
            return None;
        }

        // overflow
        if self.cur >= self.queries.0.len() as isize {
            return None;
        }

        let data: &[u8] = &self.queries.0[self.cur as usize..];
        match data.iter().position(|b| *b == b'&') {
            Some(to) => {
                self.cur += to as isize + 1;
                let data = &data[..to];
                if data.is_empty() {
                    self.next()
                } else {
                    Some(Query(data))
                }
            }
            None => {
                self.cur = -1;
                if data.is_empty() {
                    self.next()
                } else {
                    Some(Query(data))
                }
            }
        }
    }
}

#[cfg(test)]
mod queries_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn queries() {
        init();

        let raw = b"foo=1&&x=2&details";
        let queries = Queries::from(&raw[..]);

        let x = queries.get("x");
        assert!(x.is_some());
        let x = x.unwrap();
        assert_eq!("x", &x.key());
        assert_eq!("2", &x.value().unwrap());

        for q in queries.iter() {
            let k = q.key();
            let v = q.value();

            info!("next query: |{}|{}|{:?}|", q.as_str(), k, v);

            match k.as_ref() {
                "foo" => assert_eq!(Some("1".into()), v),
                "x" => assert_eq!(Some("2".into()), v),
                "details" => assert!(v.is_none()),
                k => {
                    error!("bad key {}", k)
                }
            }
        }
    }

    #[test]
    fn test_weird_values() {
        init();

        let raw = b"x=a=1;b=2";
        let queries = Queries::from(&raw[..]);
    }

    #[test]
    fn test_values() {
        init();

        let raw = b"x=a&y=2&x=&x=b&z=3&x=c&x";
        let queries = Queries::from(&raw[..]);

        let found = queries.get_all("x");
        assert_eq!(5, found.len());

        for next in found.iter() {
            info!("next: |{}|", String::from_utf8_lossy(next));
        }

        let mut it = found.into_iter();
        assert_eq!(Some(&b"a"[..]), it.next());
        assert_eq!(Some(&b""[..]), it.next());
        assert_eq!(Some(&b"b"[..]), it.next());
        assert_eq!(Some(&b"c"[..]), it.next());
        assert_eq!(Some(&b""[..]), it.next());
        assert!(it.next().is_none());

        let first_x = queries.get_first("x");
        assert_eq!(Some(&b"a"[..]), first_x);

        let last_x = queries.get_last("x");
        assert_eq!(Some(&b""[..]), last_x);

        let y = queries.get_all("y");
        assert_eq!(1, y.len());
        let mut it = y.into_iter();
        assert_eq!(Some(&b"2"[..]), it.next());
        assert!(it.next().is_none());

        let z = queries.get_all("z");
        assert_eq!(1, z.len());
        let mut it = z.into_iter();
        assert_eq!(Some(&b"3"[..]), it.next());
        assert!(it.next().is_none());

        let last_z = queries.get_last("z");
        assert_eq!(Some(&b"3"[..]), last_z);
    }
}
