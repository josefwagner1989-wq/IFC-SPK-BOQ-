//! Matemática mínima de vetores/matrizes.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl V3 {
    pub fn new(x: f64, y: f64, z: f64) -> V3 { V3 { x, y, z } }
    pub fn add(&self, o: &V3) -> V3 { V3::new(self.x + o.x, self.y + o.y, self.z + o.z) }
    pub fn sub(&self, o: &V3) -> V3 { V3::new(self.x - o.x, self.y - o.y, self.z - o.z) }
    pub fn scale(&self, s: f64) -> V3 { V3::new(self.x * s, self.y * s, self.z * s) }
    pub fn dot(&self, o: &V3) -> f64 { self.x * o.x + self.y * o.y + self.z * o.z }
    pub fn cross(&self, o: &V3) -> V3 {
        V3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(&self) -> f64 { self.dot(self).sqrt() }
    pub fn norm(&self) -> V3 {
        let l = self.len();
        if l > 1e-12 { self.scale(1.0 / l) } else { *self }
    }
    pub fn dist(&self, o: &V3) -> f64 { self.sub(o).len() }
}

/// Matriz afim 3x4, colunas = eixo X, Y, Z, origem.
#[derive(Debug, Clone, Copy)]
pub struct Mat4 {
    pub x: V3,
    pub y: V3,
    pub z: V3,
    pub o: V3,
}

impl Mat4 {
    pub fn identity() -> Mat4 {
        Mat4 { x: V3::new(1.0, 0.0, 0.0), y: V3::new(0.0, 1.0, 0.0), z: V3::new(0.0, 0.0, 1.0), o: V3::default() }
    }
    /// Posicionamento de eixos IFC: eixo Z + direção de referência (X), ortonormalizado.
    pub fn from_axes(o: V3, z: V3, xref: V3) -> Mat4 {
        let z = z.norm();
        let mut x = xref.sub(&z.scale(xref.dot(&z)));
        if x.len() < 1e-9 {
            x = if z.x.abs() < 0.9 { V3::new(1.0, 0.0, 0.0) } else { V3::new(0.0, 1.0, 0.0) };
            x = x.sub(&z.scale(x.dot(&z)));
        }
        let x = x.norm();
        let y = z.cross(&x);
        Mat4 { x, y, z, o }
    }
    pub fn scaled(&self, s: f64) -> Mat4 {
        Mat4 { x: self.x.scale(s), y: self.y.scale(s), z: self.z.scale(s), o: self.o }
    }
    pub fn apply(&self, p: V3) -> V3 {
        self.o.add(&self.x.scale(p.x)).add(&self.y.scale(p.y)).add(&self.z.scale(p.z))
    }
    pub fn apply_dir(&self, p: V3) -> V3 {
        self.x.scale(p.x).add(&self.y.scale(p.y)).add(&self.z.scale(p.z))
    }
    pub fn mul(&self, b: &Mat4) -> Mat4 {
        Mat4 { x: self.apply_dir(b.x), y: self.apply_dir(b.y), z: self.apply_dir(b.z), o: self.apply(b.o) }
    }
    /// Inversa para transformações ortonormais (rígidas).
    pub fn inverse_rigid(&self) -> Mat4 {
        let x = V3::new(self.x.x, self.y.x, self.z.x);
        let y = V3::new(self.x.y, self.y.y, self.z.y);
        let z = V3::new(self.x.z, self.y.z, self.z.z);
        let r = Mat4 { x, y, z, o: V3::default() };
        let o = r.apply_dir(self.o).scale(-1.0);
        Mat4 { o, ..r }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inv() {
        let m = Mat4::from_axes(V3::new(1.0, 2.0, 3.0), V3::new(0.0, 1.0, 0.0), V3::new(1.0, 0.0, 0.0));
        let p = V3::new(4.0, 5.0, 6.0);
        let q = m.inverse_rigid().apply(m.apply(p));
        assert!(q.dist(&p) < 1e-9);
    }
}
