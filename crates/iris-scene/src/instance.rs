//! 96-byte instance record: model matrix, tint, and a UV rectangle.

/// One instanced quad. The matrix is column-major, matching WGSL `mat4x4`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleInstance {
    /// Columns of the model matrix.
    pub model_matrix: [[f32; 4]; 4],
    /// Straight RGBA tint.
    pub color_tint: [f32; 4],
    /// `u0, v0, u1, v1`.
    pub tex_coords: [f32; 4],
}

/// A centered quad `width` by `height` pixels at `(x, y)`.
pub fn quad_instance(x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) -> ParticleInstance {
    let model = glam::Mat4::from_scale_rotation_translation(
        glam::Vec3::new(width, height, 1.0),
        glam::Quat::IDENTITY,
        glam::Vec3::new(x, y, 0.0),
    );
    ParticleInstance {
        model_matrix: model.to_cols_array_2d(),
        color_tint: color,
        tex_coords: [0.0, 0.0, 1.0, 1.0],
    }
}

pub fn push_bytes(instances: &[ParticleInstance], out: &mut Vec<u8>) {
    out.extend_from_slice(bytemuck::cast_slice(instances));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_is_96_bytes() {
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 96);
        let instance = quad_instance(3.0, 4.0, 10.0, 20.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(instance.model_matrix[3][0], 3.0);
        assert_eq!(instance.model_matrix[3][1], 4.0);
        assert_eq!(instance.model_matrix[0][0], 10.0);
        assert_eq!(instance.model_matrix[1][1], 20.0);
    }
}
