pub struct WeightedResource<T>(Vec<(u64, T)>);

impl<T> WeightedResource<T> {
    pub fn builder() -> WeightedResourceBuilder<T> {
        WeightedResourceBuilder { inner: vec![] }
    }

    pub fn next(&self) -> Option<&T> {
        match self.0.last() {
            None => None,
            Some((max, _)) => {
                let seed = {
                    use rand::prelude::*;

                    let mut rng = thread_rng();
                    rng.gen_range(0..*max)
                };

                let idx = match self.0.binary_search_by_key(&seed, |&(weight, _)| weight) {
                    Ok(i) => i + 1,
                    Err(i) => i,
                };

                self.0.get(idx).map(|(_, u)| u)
            }
        }
    }
}

pub struct WeightedResourceBuilder<T> {
    inner: Vec<(u64, T)>,
}

impl<T> WeightedResourceBuilder<T> {
    pub fn push(self, weight: u32, resource: T) -> Self {
        let Self { mut inner } = self;

        if weight != 0 {
            let w = weight as u64 + inner.last().map(|(w, _)| *w).unwrap_or_default();
            inner.push((w, resource));
        }

        Self { inner }
    }

    pub fn build(self) -> WeightedResource<T> {
        WeightedResource(self.inner)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_weighted_resource() {
        init();

        let wr = WeightedResource::<&str>::builder()
            .push(2, "foo")
            .push(3, "bar")
            .push(5, "qux")
            .build();

        let mut cnts: HashMap<&str, usize> = Default::default();
        const N: usize = 1000000;

        (0..N).for_each(|_| {
            let k = wr.next().unwrap();
            let val = cnts.entry(*k).or_insert(0usize);
            *val = *val + 1;
        });

        for (k, v) in cnts {
            println!("{}: {:.2}%", k, 100f64 * (v as f64) / (N as f64));
        }
    }
}
