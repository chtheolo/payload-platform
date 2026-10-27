/// Optical zoom expressed as a magnification factor, where 1.0 is fully zoomed out.
#[derive(Debug, Clone)]
pub struct Zoom {
    factor: f32,
    max_factor: f32,
}

impl Zoom {
    pub fn new(max_factor: f32) -> Self {
        Self {
            factor: 1.0,
            max_factor: max_factor.max(1.0),
        }
    }

    pub fn factor(&self) -> f32 {
        self.factor
    }

    /// Position within the zoom range: 0.0 fully out, 1.0 fully in.
    pub fn fraction(&self) -> f32 {
        (self.factor - 1.0) / (self.max_factor - 1.0).max(f32::EPSILON)
    }

    /// Sets the zoom to `factor`, clamped to the lens range.
    pub fn set_factor(&mut self, factor: f32) {
        self.factor = factor.clamp(1.0, self.max_factor);
    }

    pub fn set_fraction(&mut self, fraction: f32) {
        self.set_factor(1.0 + fraction.clamp(0.0, 1.0) * (self.max_factor - 1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_factor_clamps_to_the_lens_range() {
        let mut zoom = Zoom::new(6.0);
        zoom.set_factor(20.0);
        assert_eq!(zoom.factor(), 6.0);
        zoom.set_factor(0.5);
        assert_eq!(zoom.factor(), 1.0);
    }

    #[test]
    fn fraction_round_trips() {
        let mut zoom = Zoom::new(5.0);
        zoom.set_fraction(0.5);
        assert_eq!(zoom.factor(), 3.0);
        assert_eq!(zoom.fraction(), 0.5);
    }
}
