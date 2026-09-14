//! Place, arrange, auto-orient, and lay-on-face for instances on a bed.

use bambu_config::BedShape;
use bambu_geom::TriangleMesh;
use glam::{Mat3, Quat, Vec3};

use crate::{Instance, Model};

impl Model {
    /// Center identity instances that still sit at the origin (STL / cube),
    /// and drop every instance so it rests on z=0. 3MF layouts that already
    /// span the plate are left in XY.
    pub fn place_on_bed_if_needed(&mut self, bed: &BedShape) {
        let Some(merged) = self.merged_mesh() else {
            return;
        };
        let Some(aabb) = merged.aabb() else {
            return;
        };
        let (x0, y0, x1, y1) = bed.printable_aabb();
        let plate_area = (x1 - x0).max(1.0) * (y1 - y0).max(1.0);
        let obj_area = aabb.size().x.max(0.0) * aabb.size().y.max(0.0);
        let near_origin = aabb.min.x < 1.0 && aabb.min.y < 1.0;
        let small = obj_area < plate_area * 0.25;
        let all_identity = self
            .objects
            .iter()
            .all(|o| o.instances.iter().all(|i| i.is_identity()));
        let center = near_origin && small && all_identity;

        for obj in &mut self.objects {
            let mesh = obj.printable_mesh();
            let Some(oa) = mesh.aabb() else {
                continue;
            };
            if obj.instances.is_empty() {
                obj.instances.push(Instance::default());
            }
            for inst in &mut obj.instances {
                if center && inst.is_identity() {
                    let size = oa.size();
                    inst.offset.x = x0 + (x1 - x0 - size.x) * 0.5 - oa.min.x;
                    inst.offset.y = y0 + (y1 - y0 - size.y) * 0.5 - oa.min.y;
                    inst.offset.z = -oa.min.z;
                } else {
                    drop_instance_to_bed(&mesh, inst);
                }
            }
        }
    }

    /// Pack object AABBs with a gap, then translate the pile so its center
    /// matches the bed center (`ArrangeParams::do_final_align` + `align_center`
    /// `{0.5, 0.5}` in Bambu Studio). A single object therefore stays centered.
    pub fn arrange_on_plate(&mut self, plate: usize, bed: &BedShape, gap: f32) {
        let (x0, y0, x1, y1) = bed.printable_aabb();
        let indices: Vec<usize> = match self.plates.get(plate) {
            Some(p) => p.object_indices.clone(),
            None => (0..self.objects.len()).collect(),
        };
        let gap = gap.max(1.0);
        for &i in &indices {
            if let Some(obj) = self.objects.get_mut(i) {
                if obj.instances.is_empty() {
                    obj.instances.push(Instance::default());
                }
            }
        }

        struct Item {
            object: usize,
            instance: usize,
            w: f32,
            d: f32,
            min_x: f32,
            min_y: f32,
            min_z: f32,
        }
        let mut items = Vec::new();
        for &i in &indices {
            let Some(obj) = self.objects.get(i) else {
                continue;
            };
            let mesh = obj.printable_mesh();
            for (j, inst) in obj.instances.iter().enumerate() {
                let local = {
                    let mut without_xy = *inst;
                    without_xy.offset = Vec3::ZERO;
                    without_xy.apply_to_mesh(&mesh)
                };
                let Some(aabb) = local.aabb() else {
                    continue;
                };
                items.push(Item {
                    object: i,
                    instance: j,
                    w: aabb.size().x,
                    d: aabb.size().y,
                    min_x: aabb.min.x,
                    min_y: aabb.min.y,
                    min_z: aabb.min.z,
                });
            }
        }

        let mut x = x0 + 1.0;
        let mut y = y0 + 1.0;
        let mut row_h = 0.0_f32;
        let mut placed = Vec::new();
        for item in items {
            if x > x0 + 1.5 && x + item.w > x1 - 1.0 {
                x = x0 + 1.0;
                y += row_h + gap;
                row_h = 0.0;
            }
            if y + item.d > y1 {
                break;
            }
            placed.push((
                item.object,
                item.instance,
                x - item.min_x,
                y - item.min_y,
                -item.min_z,
            ));
            x += item.w + gap;
            row_h = row_h.max(item.d);
        }

        for &(object, instance, ox, oy, oz) in &placed {
            if let Some(inst) = self
                .objects
                .get_mut(object)
                .and_then(|o| o.instances.get_mut(instance))
            {
                inst.offset.x = ox;
                inst.offset.y = oy;
                inst.offset.z = oz;
            }
        }

        let mut pile = None;
        for &(object, instance, ..) in &placed {
            let Some(obj) = self.objects.get(object) else {
                continue;
            };
            let Some(inst) = obj.instances.get(instance) else {
                continue;
            };
            let Some(aabb) = inst.apply_to_mesh(&obj.printable_mesh()).aabb() else {
                continue;
            };
            pile = Some(match pile {
                Some(prev) => aabb.union(prev),
                None => aabb,
            });
        }
        let Some(pile) = pile else {
            return;
        };
        let (bed_cx, bed_cy) = bed.center();
        let pile_cx = (pile.min.x + pile.max.x) * 0.5;
        let pile_cy = (pile.min.y + pile.max.y) * 0.5;
        let mut dx = bed_cx - pile_cx;
        let mut dy = bed_cy - pile_cy;
        if pile.min.x + dx < x0 {
            dx = x0 - pile.min.x;
        }
        if pile.max.x + dx > x1 {
            dx = x1 - pile.max.x;
        }
        if pile.min.y + dy < y0 {
            dy = y0 - pile.min.y;
        }
        if pile.max.y + dy > y1 {
            dy = y1 - pile.max.y;
        }
        for &(object, instance, ..) in &placed {
            if let Some(inst) = self
                .objects
                .get_mut(object)
                .and_then(|o| o.instances.get_mut(instance))
            {
                inst.offset.x += dx;
                inst.offset.y += dy;
            }
        }
    }
}

pub fn drop_instance_to_bed(mesh: &TriangleMesh, inst: &mut Instance) {
    let world = inst.apply_to_mesh(mesh);
    if let Some(aabb) = world.aabb() {
        inst.offset.z -= aabb.min.z;
    }
}

pub fn auto_orient_instance(mesh: &TriangleMesh, inst: &mut Instance) {
    let mut best_rot = inst.rotation_deg;
    let mut best_h = f32::MAX;
    for down in [
        Vec3::ZERO,
        Vec3::new(180.0, 0.0, 0.0),
        Vec3::new(90.0, 0.0, 0.0),
        Vec3::new(-90.0, 0.0, 0.0),
        Vec3::new(0.0, 90.0, 0.0),
        Vec3::new(0.0, -90.0, 0.0),
    ] {
        for z in [0.0_f32, 90.0, 180.0, 270.0] {
            let mut trial = *inst;
            trial.rotation_deg = Vec3::new(down.x, down.y, down.z + z);
            let Some(aabb) = trial.apply_to_mesh(mesh).aabb() else {
                continue;
            };
            let h = aabb.size().z;
            if h + 1e-4 < best_h {
                best_h = h;
                best_rot = trial.rotation_deg;
            }
        }
    }
    inst.rotation_deg = best_rot;
    drop_instance_to_bed(mesh, inst);
}

pub fn lay_instance_on_normal(mesh: &TriangleMesh, inst: &mut Instance, world_normal: Vec3) {
    let n = world_normal.normalize_or_zero();
    if n.length_squared() < 1e-8 {
        return;
    }
    let r_add = Mat3::from_quat(Quat::from_rotation_arc(n, Vec3::Z));
    let r_cur = Mat3::from_euler(
        glam::EulerRot::XYZ,
        inst.rotation_deg.x.to_radians(),
        inst.rotation_deg.y.to_radians(),
        inst.rotation_deg.z.to_radians(),
    );
    let (rx, ry, rz) = (r_add * r_cur).to_euler(glam::EulerRot::XYZ);
    inst.rotation_deg = Vec3::new(rx.to_degrees(), ry.to_degrees(), rz.to_degrees());
    drop_instance_to_bed(mesh, inst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Model;
    use bambu_geom::TriangleMesh;

    #[test]
    fn arrange_two_cubes_do_not_overlap() {
        let mut model = Model::from_mesh("a", TriangleMesh::cube(20.0));
        model
            .objects
            .push(crate::ModelObject::new("b", TriangleMesh::cube(20.0)));
        model.plates[0].object_indices = vec![0, 1];
        let bed = BedShape::square(256.0);
        model.arrange_on_plate(0, &bed, 5.0);
        let a = model.objects[0].instances[0]
            .apply_to_mesh(&model.objects[0].mesh)
            .aabb()
            .unwrap();
        let b = model.objects[1].instances[0]
            .apply_to_mesh(&model.objects[1].mesh)
            .aabb()
            .unwrap();
        let overlap_x = a.min.x < b.max.x && b.min.x < a.max.x;
        let overlap_y = a.min.y < b.max.y && b.min.y < a.max.y;
        assert!(
            !(overlap_x && overlap_y),
            "arranged AABBs overlap {a:?} {b:?}"
        );
        assert!(a.min.z.abs() < 1e-3 && b.min.z.abs() < 1e-3);
    }

    #[test]
    fn place_cube_centers_on_square_bed() {
        let mut model = Model::from_mesh("cube", TriangleMesh::cube(20.0));
        let bed = BedShape::square(256.0);
        model.place_on_bed_if_needed(&bed);
        let aabb = model.mesh_for_plate(0).unwrap().aabb().unwrap();
        assert!((aabb.min.z).abs() < 1e-3);
        let cx = (aabb.min.x + aabb.max.x) * 0.5;
        let cy = (aabb.min.y + aabb.max.y) * 0.5;
        assert!((cx - 128.0).abs() < 0.5, "cx {cx}");
        assert!((cy - 128.0).abs() < 0.5, "cy {cy}");
    }

    #[test]
    fn arrange_lone_cube_centers_on_square_bed() {
        let mut model = Model::from_mesh("cube", TriangleMesh::cube(20.0));
        let bed = BedShape::square(256.0);
        model.arrange_on_plate(0, &bed, 8.0);
        let aabb = model.mesh_for_plate(0).unwrap().aabb().unwrap();
        assert!(aabb.min.z.abs() < 1e-3);
        let cx = (aabb.min.x + aabb.max.x) * 0.5;
        let cy = (aabb.min.y + aabb.max.y) * 0.5;
        assert!((cx - 128.0).abs() < 0.5, "cx {cx}");
        assert!((cy - 128.0).abs() < 0.5, "cy {cy}");
    }

    #[test]
    fn auto_orient_lowers_a_tall_box() {
        let mesh = TriangleMesh::box_mm(10.0, 8.0, 40.0);
        let mut inst = Instance::default();
        auto_orient_instance(&mesh, &mut inst);
        let aabb = inst.apply_to_mesh(&mesh).aabb().unwrap();
        assert!(
            aabb.size().z <= 10.0 + 1e-3,
            "expected a face-down 10mm height, got {}",
            aabb.size().z
        );
        assert!(aabb.min.z.abs() < 1e-3);
    }
}
