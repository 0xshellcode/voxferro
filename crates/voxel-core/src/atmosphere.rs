//! Physically based sky: single scattering through a spherical atmosphere.
//!
//! The model's parameters:
//!
//! * scattering `β_ray = ray_k · 0.05 · (10^(1.5·ray_d) − 1)`,
//!   `β_mie = mie_k · (10^(1.5·mie_d) − 1) / 300`, and an ozone term with the
//!   Rayleigh scale that only absorbs;
//! * extinction `β_ray + 1.1·β_mie + β_ozone`;
//! * a planet of radius 10 000 with the top of the atmosphere at
//!   `R + ln(1 + 10⁶ · β_max)`.
//!
//! That logarithm pins down the density profile: `e^(−altitude)` with a scale
//! height of 1 puts the top exactly where density times the strongest
//! coefficient falls to one millionth. The integration below (single
//! scattering, Rayleigh and Cornette-Shanks phases) is a standard one built
//! on those parameters.

use crate::EnvMap;
use glam::Vec3;

pub const PLANET_RADIUS: f32 = 10_000.0;

/// Overall scale of the computed sky against the sun.
///
/// Calibrated against a reference render of the same scene, using ratios
/// that cancel exposure: sky against sunlit ground, and shadow against
/// sunlit ground. Both put the factor between roughly 4 and 7; 2π sits in
/// that range. An earlier match on absolute pixel values pointed at 4π, but
/// that relied on an exposure this renderer had guessed, and is discarded.
pub const SKY_SCALE: f32 = std::f32::consts::TAU;

/// `_atm` settings as stored in a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtmosphereSettings {
    /// Colours as 0..1 RGB.
    pub ray_color: Vec3,
    pub ray_density: f32,
    pub mie_color: Vec3,
    pub mie_density: f32,
    pub mie_g: f32,
    pub ozone_color: Vec3,
    pub ozone_density: f32,
}

impl Default for AtmosphereSettings {
    /// What the editor uses for a file without `_atm`: its sky panel shows
    /// Rayleigh 40, Mie 40 (g 0.85), ozone 0. Every sample file happens to
    /// save `_ray_d 0.6` instead, but that is a saved value, not the default.
    fn default() -> Self {
        Self {
            ray_color: Vec3::new(45.0, 104.0, 255.0) / 255.0,
            ray_density: 0.4,
            mie_color: Vec3::ONE,
            mie_density: 0.4,
            mie_g: 0.85,
            ozone_color: Vec3::new(105.0, 255.0, 110.0) / 255.0,
            ozone_density: 0.0,
        }
    }
}

/// Coefficients derived from the settings, per unit length.
#[derive(Debug, Clone, Copy)]
pub struct Atmosphere {
    pub rayleigh: Vec3,
    pub mie: Vec3,
    pub ozone: Vec3,
    pub extinction: Vec3,
    pub mie_g: f32,
    pub top_radius: f32,
}

fn density_curve(d: f32) -> f32 {
    10f32.powf(1.5 * d) - 1.0
}

impl Atmosphere {
    pub fn new(s: &AtmosphereSettings) -> Self {
        let clamp = |c: Vec3| c.clamp(Vec3::ZERO, Vec3::ONE);
        let rayleigh = clamp(s.ray_color) * 0.05 * density_curve(s.ray_density);
        let mie = clamp(s.mie_color) * density_curve(s.mie_density) / 300.0;
        let ozone = clamp(s.ozone_color) * 0.05 * density_curve(s.ozone_density);
        let strongest = rayleigh.max_element().max(mie.max_element()).max(0.0);
        Self {
            rayleigh,
            mie,
            ozone,
            extinction: rayleigh + mie * 1.1 + ozone,
            mie_g: s.mie_g.clamp(-0.999, 0.999),
            top_radius: PLANET_RADIUS + (1.0 + strongest * 1e6).ln(),
        }
    }

    fn density(&self, p: Vec3) -> f32 {
        (-(p.length() - PLANET_RADIUS).max(0.0)).exp()
    }

    /// Distance along `dir` from `origin` to the top of the atmosphere, or
    /// `None` when the ray meets the planet first.
    fn path_length(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        // Planet hit, for rays below the horizon.
        let b = origin.dot(dir);
        let c_planet = origin.length_squared() - PLANET_RADIUS * PLANET_RADIUS;
        let disc_planet = b * b - c_planet;
        if disc_planet >= 0.0 && b < 0.0 {
            let t = -b - disc_planet.sqrt();
            if t > 1e-3 {
                return None;
            }
        }
        let c_top = origin.length_squared() - self.top_radius * self.top_radius;
        let disc = b * b - c_top;
        if disc < 0.0 {
            return Some(0.0);
        }
        Some((-b + disc.sqrt()).max(0.0))
    }

    /// Integral of density along a ray, out to the top of the atmosphere.
    fn optical_depth(&self, origin: Vec3, dir: Vec3, steps: u32) -> Option<f32> {
        let length = self.path_length(origin, dir)?;
        let ds = length / steps as f32;
        let mut depth = 0.0;
        for i in 0..steps {
            depth += self.density(origin + dir * ((i as f32 + 0.5) * ds)) * ds;
        }
        Some(depth)
    }

    /// Fraction of light that crosses the whole atmosphere to reach the
    /// ground from `dir`. This is what reddens a low sun.
    pub fn transmittance(&self, dir: Vec3) -> Vec3 {
        let origin = Vec3::new(0.0, 0.0, PLANET_RADIUS + 1e-3);
        match self.optical_depth(origin, dir.normalize_or(Vec3::Z), 64) {
            Some(depth) => (-self.extinction * depth).exp(),
            None => Vec3::ZERO,
        }
    }

    /// Single-scattered sky radiance toward the viewer from direction `view`,
    /// per unit of sun irradiance arriving from `sun`.
    pub fn sky_radiance(&self, view: Vec3, sun: Vec3) -> Vec3 {
        let view = view.normalize_or(Vec3::Z);
        let sun = sun.normalize_or(Vec3::Z);
        let origin = Vec3::new(0.0, 0.0, PLANET_RADIUS + 1e-3);

        // Below the horizon the view ray ends on the planet; integrate up to
        // there so the sky still glows down to the horizon line.
        let b = origin.dot(view);
        let disc = b * b - (origin.length_squared() - PLANET_RADIUS * PLANET_RADIUS);
        let to_ground = if disc >= 0.0 && b < 0.0 { Some(-b - disc.sqrt()) } else { None };
        let to_top = {
            let d = b * b - (origin.length_squared() - self.top_radius * self.top_radius);
            (-b + d.max(0.0).sqrt()).max(0.0)
        };
        let length = to_ground.filter(|&t| t > 1e-3).unwrap_or(to_top).min(to_top);

        let mu = view.dot(sun);
        let rayleigh_phase = 3.0 / (16.0 * std::f32::consts::PI) * (1.0 + mu * mu);
        let g = self.mie_g;
        let mie_phase = 3.0 / (8.0 * std::f32::consts::PI) * (1.0 - g * g) * (1.0 + mu * mu)
            / ((2.0 + g * g) * (1.0 + g * g - 2.0 * g * mu).powf(1.5));

        const STEPS: u32 = 48;
        let ds = length / STEPS as f32;
        let mut view_depth = 0.0;
        let mut sum = Vec3::ZERO;
        for i in 0..STEPS {
            let p = origin + view * ((i as f32 + 0.5) * ds);
            let rho = self.density(p);
            view_depth += rho * ds * 0.5;
            if let Some(sun_depth) = self.optical_depth(p, sun, 12) {
                let attenuation = (-self.extinction * (view_depth + sun_depth)).exp();
                sum += attenuation
                    * (self.rayleigh * rayleigh_phase + self.mie * mie_phase)
                    * rho
                    * ds;
            }
            view_depth += rho * ds * 0.5;
        }
        sum * SKY_SCALE
    }

    /// Renders the sky into an equirectangular map, in the same orientation
    /// convention the path tracer reads environment maps with: row 0 up,
    /// azimuth clockwise seen from above.
    pub fn sky_map(&self, sun: Vec3, width: u32, height: u32) -> EnvMap {
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            let theta = std::f32::consts::PI * (y as f32 + 0.5) / height as f32;
            let (sin_t, cos_t) = theta.sin_cos();
            for x in 0..width {
                let phi = std::f32::consts::TAU * (x as f32 + 0.5) / width as f32;
                let dir = Vec3::new(sin_t * phi.cos(), -sin_t * phi.sin(), cos_t);
                pixels.push(self.sky_radiance(dir, sun).to_array());
            }
        }
        EnvMap {
            width,
            height,
            pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atmosphere() -> Atmosphere {
        Atmosphere::new(&AtmosphereSettings::default())
    }

    #[test]
    fn coefficients_follow_the_editor_formulas() {
        let a = atmosphere();
        // ray_d 0.4: 0.05 * (10^0.6 - 1), times the blue channel 1.0.
        assert!((a.rayleigh.z - 0.05 * (10f32.powf(0.6) - 1.0)).abs() < 1e-5);
        // o3_d 0 contributes nothing.
        assert_eq!(a.ozone, Vec3::ZERO);
        assert!((a.extinction.x - (a.rayleigh.x + 1.1 * a.mie.x)).abs() < 1e-6);
        assert!(a.top_radius > PLANET_RADIUS && a.top_radius < PLANET_RADIUS + 20.0);
    }

    #[test]
    fn a_clear_sky_is_blue() {
        let sky = atmosphere().sky_radiance(Vec3::Z, Vec3::new(0.6, 0.0, 0.8));
        assert!(sky.z > sky.x, "zenith should be bluer than it is red: {sky:?}");
    }

    #[test]
    fn a_low_sun_is_redder_than_a_high_one() {
        let a = atmosphere();
        let noon = a.transmittance(Vec3::Z);
        let dusk = a.transmittance(Vec3::new(1.0, 0.0, 0.03));
        assert!(dusk.length() < noon.length(), "more air in the way at dusk");
        assert!(dusk.x / dusk.z > noon.x / noon.z, "blue is lost first, so it reddens");
    }

    #[test]
    fn the_sky_is_brighter_toward_the_sun() {
        let a = atmosphere();
        let sun = Vec3::new(0.0, -0.8, 0.6).normalize();
        let toward = a.sky_radiance(Vec3::new(0.0, -0.75, 0.66).normalize(), sun);
        let away = a.sky_radiance(Vec3::new(0.0, 0.75, 0.66).normalize(), sun);
        assert!(toward.length() > away.length(), "forward Mie scattering");
    }

    #[test]
    fn no_air_means_no_sky() {
        let a = Atmosphere::new(&AtmosphereSettings {
            ray_density: 0.0,
            mie_density: 0.0,
            ozone_density: 0.0,
            ..Default::default()
        });
        assert_eq!(a.sky_radiance(Vec3::Z, Vec3::Z), Vec3::ZERO);
        assert_eq!(a.transmittance(Vec3::Z), Vec3::ONE);
    }
}
