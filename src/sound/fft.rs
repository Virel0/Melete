//! A radix-2 FFT, in place, ported from the sidecar's (`Fft.cs`): the spectra tempo and,
//! later, key are read from. Small enough not to need a library.

/// A radix-2 FFT of one fixed size, its tables worked out once.
pub struct Fft {
    size: usize,
    reversed: Vec<usize>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    /// None unless `size` is a power of two of at least 2.
    pub fn new(size: usize) -> Option<Self> {
        if size < 2 || !size.is_power_of_two() {
            return None;
        }
        let bits = size.trailing_zeros();
        let reversed = (0..size)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let angle = |i: usize| 2.0 * std::f64::consts::PI * i as f64 / size as f64;
        Some(Self {
            size,
            reversed,
            cos: (0..size / 2).map(|i| angle(i).cos()).collect(),
            sin: (0..size / 2).map(|i| angle(i).sin()).collect(),
        })
    }

    /// Transforms both halves in place; each must be `size` long.
    pub fn transform(&self, real: &mut [f64], imaginary: &mut [f64]) {
        assert!(real.len() == self.size && imaginary.len() == self.size);
        for i in 0..self.size {
            let j = self.reversed[i];
            if j > i {
                real.swap(i, j);
                imaginary.swap(i, j);
            }
        }
        let mut length = 2;
        while length <= self.size {
            let half = length / 2;
            let step = self.size / length;
            for start in (0..self.size).step_by(length) {
                for offset in 0..half {
                    let twiddle = offset * step;
                    let (wr, wi) = (self.cos[twiddle], -self.sin[twiddle]);
                    let (lo, hi) = (start + offset, start + offset + half);
                    let vr = real[hi] * wr - imaginary[hi] * wi;
                    let vi = real[hi] * wi + imaginary[hi] * wr;
                    real[hi] = real[lo] - vr;
                    imaginary[hi] = imaginary[lo] - vi;
                    real[lo] += vr;
                    imaginary[lo] += vi;
                }
            }
            length <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    //! The sidecar's `FftTests`.

    use super::*;

    fn buffers(size: usize, sample: impl Fn(usize) -> f64) -> (Vec<f64>, Vec<f64>) {
        ((0..size).map(sample).collect(), vec![0.0; size])
    }

    fn magnitude(real: &[f64], imaginary: &[f64], k: usize) -> f64 {
        real[k].hypot(imaginary[k])
    }

    #[test]
    fn a_sinusoid_lands_in_its_own_bin() {
        let (size, bin) = (1024, 37);
        let (mut real, mut imaginary) = buffers(size, |n| {
            (2.0 * std::f64::consts::PI * bin as f64 * n as f64 / size as f64).cos()
        });
        Fft::new(size).unwrap().transform(&mut real, &mut imaginary);
        let loudest = (1..size / 2)
            .max_by(|&a, &b| {
                magnitude(&real, &imaginary, a).total_cmp(&magnitude(&real, &imaginary, b))
            })
            .unwrap();
        assert_eq!(loudest, bin);
    }

    #[test]
    fn an_impulse_is_flat() {
        let size = 256;
        let (mut real, mut imaginary) = buffers(size, |n| if n == 0 { 1.0 } else { 0.0 });
        Fft::new(size).unwrap().transform(&mut real, &mut imaginary);
        for k in 0..size {
            assert!((magnitude(&real, &imaginary, k) - 1.0).abs() < 1e-9);
        }
    }

    /// The one that catches a wrong twiddle sign or a botched bit reversal.
    #[test]
    fn it_agrees_with_the_definition() {
        let size = 64;
        let mut random = super::super::tempo::tests::Random::new(3);
        let samples: Vec<f64> = (0..size).map(|_| random.next() * 2.0 - 1.0).collect();
        let (mut real, mut imaginary) = buffers(size, |n| samples[n]);
        Fft::new(size).unwrap().transform(&mut real, &mut imaginary);
        for k in 0..size {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, sample) in samples.iter().enumerate() {
                let angle = -2.0 * std::f64::consts::PI * (k * n) as f64 / size as f64;
                re += sample * angle.cos();
                im += sample * angle.sin();
            }
            assert!((re - real[k]).abs() < 1e-9 && (im - imaginary[k]).abs() < 1e-9);
        }
    }

    #[test]
    fn a_size_that_is_not_a_power_of_two_is_refused() {
        for size in [0, 1, 6, 1000] {
            assert!(Fft::new(size).is_none(), "{size}");
        }
    }
}
