/// 3x3 affine transform matrix using PDF's a,b,c,d,e,f notation.
#[derive(Debug, Clone, Copy)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub fn identity() -> Self {
        Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Matrix multiplication: self * other (pre-multiply)
    pub fn concat(&self, other: &Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    /// Transform a point (x, y) through this matrix.
    /// x' = a*x + c*y + e, y' = b*x + d*y + f
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Color {
    pub fn black() -> Self {
        Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        }
    }

    pub fn _white() -> Self {
        Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphicsState {
    pub ctm: Matrix,
    pub fill_color: Color,
    pub stroke_color: Color,
    pub line_width: f64,
    pub line_cap: u8,
    pub line_join: u8,
    pub miter_limit: f64,
    pub dash_array: Vec<f64>,
    pub dash_phase: f64,
    pub fill_alpha: f64,
    pub stroke_alpha: f64,
    pub font_name: Vec<u8>,
    pub font_size: f64,
    pub char_spacing: f64,
    pub word_spacing: f64,
    pub horizontal_scaling: f64,
    pub text_leading: f64,
    pub text_rise: f64,
    pub text_rendering_mode: u8,
}

impl Default for GraphicsState {
    fn default() -> Self {
        GraphicsState {
            ctm: Matrix::identity(),
            fill_color: Color::black(),
            stroke_color: Color::black(),
            line_width: 1.0,
            line_cap: 0,
            line_join: 0,
            miter_limit: 10.0,
            dash_array: Vec::new(),
            dash_phase: 0.0,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            font_name: Vec::new(),
            font_size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scaling: 100.0,
            text_leading: 0.0,
            text_rise: 0.0,
            text_rendering_mode: 0,
        }
    }
}

pub struct GraphicsStateStack {
    current: GraphicsState,
    stack: Vec<GraphicsState>,
}

impl GraphicsStateStack {
    pub fn new() -> Self {
        GraphicsStateStack {
            current: GraphicsState::default(),
            stack: Vec::new(),
        }
    }

    pub fn current(&self) -> &GraphicsState {
        &self.current
    }

    pub fn current_mut(&mut self) -> &mut GraphicsState {
        &mut self.current
    }

    pub fn save(&mut self) {
        const MAX_GRAPHICS_STACK_DEPTH: usize = 256;
        if self.stack.len() < MAX_GRAPHICS_STACK_DEPTH {
            self.stack.push(self.current.clone());
        }
    }

    pub fn restore(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        match self.stack.pop() {
            Some(state) => {
                self.current = state;
                Ok(())
            }
            None => Err("graphics state stack underflow: no saved state to restore".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform() {
        let m = Matrix::identity();
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 10.0).abs() < 1e-9);
        assert!((y - 20.0).abs() < 1e-9);
    }

    #[test]
    fn translation_matrix() {
        let m = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 50.0,
            f: 100.0,
        };
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 60.0).abs() < 1e-9);
        assert!((y - 120.0).abs() < 1e-9);
    }

    #[test]
    fn scale_matrix() {
        let m = Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 3.0,
            e: 0.0,
            f: 0.0,
        };
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 20.0).abs() < 1e-9);
        assert!((y - 60.0).abs() < 1e-9);
    }

    #[test]
    fn concat_translation_scale() {
        let translate = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 10.0,
            f: 20.0,
        };
        let scale = Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            e: 0.0,
            f: 0.0,
        };
        let combined = translate.concat(&scale);
        let (x, y) = combined.transform_point(5.0, 5.0);
        assert!((x - 30.0).abs() < 1e-9);
        assert!((y - 50.0).abs() < 1e-9);
    }

    #[test]
    fn state_save_restore() {
        let mut gss = GraphicsStateStack::new();
        gss.current_mut().fill_color = Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
        };
        gss.save();
        gss.current_mut().fill_color = Color {
            r: 0.0,
            g: 1.0,
            b: 0.0,
        };
        assert!((gss.current().fill_color.g - 1.0).abs() < 1e-9);
        gss.restore().unwrap();
        assert!((gss.current().fill_color.r - 1.0).abs() < 1e-9);
    }

    #[test]
    fn default_state() {
        let gss = GraphicsStateStack::new();
        let s = gss.current();
        assert!((s.line_width - 1.0).abs() < 1e-9);
        assert_eq!(s.line_cap, 0);
        assert_eq!(s.line_join, 0);
        assert!((s.miter_limit - 10.0).abs() < 1e-9);
        assert!((s.fill_alpha - 1.0).abs() < 1e-9);
        assert!((s.horizontal_scaling - 100.0).abs() < 1e-9);
    }

    #[test]
    fn restore_empty_stack_errors() {
        let mut gss = GraphicsStateStack::new();
        assert!(gss.restore().is_err());
    }
}
