//! Prepare-tab plate tools: move / rotate / scale / lay-on-face / arrange.

use bambu_config::BedShape;
use bambu_geom::Aabb3;
use bambu_gpu::{GizmoAxis, PlaterTool};
use bambu_model::{
    auto_orient_instance, drop_instance_to_bed, lay_instance_on_normal, Instance, Model,
};
use glam::Vec3;
use iced::widget::{button, checkbox, column, container, pick_list, row, text, text_input};
use iced::{Element, Fill};

use crate::theme;
use crate::Message;

/// Studio gizmo coordinate dropdown (`World` / `Object` / `Part`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CoordSpace {
    #[default]
    World,
    Object,
    Part,
}

impl CoordSpace {
    pub(crate) const MOVE: [Self; 2] = [Self::World, Self::Object];
    pub(crate) const SCALE: [Self; 3] = [Self::World, Self::Object, Self::Part];
}

impl std::fmt::Display for CoordSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::World => "World coordinates",
            Self::Object => "Object coordinates",
            Self::Part => "Part coordinates",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XformField {
    Pos(u8),
    RotRel(u8),
    RotAbs(u8),
    ScalePct(u8),
    Size(u8),
}

impl crate::App {
    pub(crate) fn plater_toolbar(&self) -> Element<'_, Message> {
        let tool = self.scene.tool;
        let chip = |label: &'static str, t: PlaterTool| {
            let active = tool == t;
            let color = if active {
                iced::Color::WHITE
            } else {
                theme::TEXT_MUTED
            };
            container(
                button(text(label).size(12).color(color))
                    .padding([3, 8])
                    .style(move |_, status| theme::process_tab(active, status))
                    .on_press(Message::PlaterTool(t)),
            )
            .style(move |_| {
                if active {
                    container::Style {
                        background: Some(iced::Background::Color(theme::PREPARE)),
                        border: iced::Border {
                            radius: theme::RADIUS.into(),
                            width: 0.0,
                            color: iced::Color::TRANSPARENT,
                        },
                        ..container::Style::default()
                    }
                } else {
                    container::Style::default()
                }
            })
        };
        let quiet = |label: &'static str, message: Message| {
            button(text(label).size(12))
                .padding([3, 8])
                .style(|_, status| theme::quiet(status))
                .on_press(message)
        };
        column![
            chip("Select", PlaterTool::Orbit),
            chip("Move", PlaterTool::Move),
            chip("Rotate", PlaterTool::Rotate),
            chip("Scale", PlaterTool::Scale),
            chip("Lay on face", PlaterTool::LayOnFace),
            checkbox(self.show_axes)
                .label("Axes")
                .on_toggle(Message::ShowAxes)
                .style(theme::tick),
            quiet("Arrange", Message::Arrange),
            quiet("Orient", Message::AutoOrient),
            quiet("Mirror X", Message::Mirror(0)),
            quiet("Mirror Y", Message::Mirror(1)),
            quiet("Mirror Z", Message::Mirror(2)),
            quiet("Rot 90°", Message::Rotate90),
        ]
        .spacing(4)
        .padding([4, 4])
        .into()
    }

    pub(crate) fn plate_hint(&self) -> String {
        if let Some((left, _)) = self.scene.bed.extruder_only_rects() {
            if left.w >= 5.0 {
                return format!(
                    "Right-drag orbit · Middle-drag pan · strip = left nozzle only ({:.0} mm)",
                    left.w
                );
            }
        }
        "Right-drag: orbit · Middle-drag: pan · Scroll: zoom · Left-drag: tool".into()
    }

    pub(crate) fn transform_panel(&self) -> Element<'_, Message> {
        let body = match self.scene.tool {
            PlaterTool::Orbit => column![
                text("On plate").size(16),
                text("Select a tool, or click the active tool again to deselect.").size(12),
            ]
            .spacing(4),
            PlaterTool::Move => self.move_panel(),
            PlaterTool::Rotate => self.rotate_panel(),
            PlaterTool::Scale => self.scale_panel(),
            PlaterTool::LayOnFace => column![
                text("Lay on face").size(16),
                text("Click a triangle to put that face on the bed.").size(12),
            ]
            .spacing(4),
        };
        body.into()
    }

    fn move_panel(&self) -> iced::widget::Column<'_, Message> {
        let space = if self.coord_space == CoordSpace::Part {
            CoordSpace::Object
        } else {
            self.coord_space
        };
        let pos_label = if space == CoordSpace::Object {
            "Translate (relative)"
        } else {
            "Position"
        };
        column![
            text("Move").size(16),
            pick_list(
                CoordSpace::MOVE.as_slice(),
                Some(space),
                Message::CoordSpace
            )
            .style(theme::choice)
            .menu_style(theme::menu),
            text(pos_label).size(12),
            axis_input("X", &self.pos_edit[0], XformField::Pos(0), "mm"),
            axis_input("Y", &self.pos_edit[1], XformField::Pos(1), "mm"),
            axis_input("Z", &self.pos_edit[2], XformField::Pos(2), "mm"),
            button(text("Drop to bed").size(12))
                .style(|_, status| theme::quiet(status))
                .on_press(Message::DropToBed),
            text("Enter applies a typed value.").size(11),
        ]
        .spacing(4)
    }

    fn rotate_panel(&self) -> iced::widget::Column<'_, Message> {
        column![
            text("Rotate").size(16),
            text("World coordinates").size(12),
            text("Rotate (relative)").size(12),
            axis_input("X", &self.rot_rel_edit[0], XformField::RotRel(0), "°"),
            axis_input("Y", &self.rot_rel_edit[1], XformField::RotRel(1), "°"),
            axis_input("Z", &self.rot_rel_edit[2], XformField::RotRel(2), "°"),
            text("Rotate (absolute)").size(12),
            axis_input("X", &self.rot_abs_edit[0], XformField::RotAbs(0), "°"),
            axis_input("Y", &self.rot_abs_edit[1], XformField::RotAbs(1), "°"),
            axis_input("Z", &self.rot_abs_edit[2], XformField::RotAbs(2), "°"),
            button(text("Reset rotation").size(12))
                .style(|_, status| theme::quiet(status))
                .on_press(Message::ResetRotation),
            text("Enter applies a typed value.").size(11),
        ]
        .spacing(4)
    }

    fn scale_panel(&self) -> iced::widget::Column<'_, Message> {
        column![
            text("Scale").size(16),
            pick_list(
                CoordSpace::SCALE.as_slice(),
                Some(self.coord_space),
                Message::CoordSpace
            )
            .style(theme::choice)
            .menu_style(theme::menu),
            text("Scale").size(12),
            axis_input("X", &self.scale_pct_edit[0], XformField::ScalePct(0), "%"),
            axis_input("Y", &self.scale_pct_edit[1], XformField::ScalePct(1), "%"),
            axis_input("Z", &self.scale_pct_edit[2], XformField::ScalePct(2), "%"),
            text("Size").size(12),
            axis_input("X", &self.size_edit[0], XformField::Size(0), "mm"),
            axis_input("Y", &self.size_edit[1], XformField::Size(1), "mm"),
            axis_input("Z", &self.size_edit[2], XformField::Size(2), "mm"),
            checkbox(self.uniform_scale)
                .label("uniform scale")
                .on_toggle(Message::UniformScale)
                .style(theme::tick),
            button(text("Reset scale").size(12))
                .style(|_, status| theme::quiet(status))
                .on_press(Message::ResetScale),
            text("Enter applies a typed value.").size(11),
        ]
        .spacing(4)
    }

    pub(crate) fn apply_plater_tool(&mut self, tool: PlaterTool) {
        if tool != PlaterTool::Orbit && self.scene.tool == tool {
            self.scene.tool = PlaterTool::Orbit;
        } else {
            self.scene.tool = tool;
        }
        if self.scene.tool != PlaterTool::Orbit {
            self.paint_kind = None;
        }
        if self.scene.tool == PlaterTool::Move && self.coord_space == CoordSpace::Part {
            self.coord_space = CoordSpace::Object;
        }
        if self.scene.tool == PlaterTool::Rotate {
            self.rotate_open_deg = self
                .model
                .as_ref()
                .and_then(|m| m.selected_instance(self.selected_object))
                .map(|i| i.rotation_deg)
                .unwrap_or(Vec3::ZERO);
        }
        self.status = match self.scene.tool {
            PlaterTool::Orbit => {
                "select: right-drag orbit · middle-drag pan · click Move/Rotate/Scale".into()
            }
            PlaterTool::Move => "move: left-drag on the plate · middle-drag pans".into(),
            PlaterTool::Rotate => "rotate: left-drag (Z) · middle-drag pans".into(),
            PlaterTool::Scale => "scale: left-drag · middle-drag pans".into(),
            PlaterTool::LayOnFace => "click a triangle to lay on the plate".into(),
        };
        self.fill_xform_edits();
    }

    pub(crate) fn handle_viewport(&mut self, event: bambu_gpu::ViewportEvent) {
        use bambu_gpu::ViewportEvent;
        match event {
            ViewportEvent::Orbit { dx, dy } => self.scene.camera.orbit(dx, dy),
            ViewportEvent::Pan { world_x, world_y } => self.scene.camera.pan_xy(world_x, world_y),
            ViewportEvent::PanScreen { dx, dy } => self.scene.camera.pan_screen(dx, dy),
            ViewportEvent::Zoom(delta) => {
                self.scene.camera.zoom(delta);
                self.sync_gizmo();
            }
            ViewportEvent::Click {
                ndc_x,
                ndc_y,
                aspect,
            } => {
                if self.scene.tool == PlaterTool::LayOnFace {
                    self.lay_on_face_pick(ndc_x, ndc_y, aspect);
                } else {
                    self.paint_pick(ndc_x, ndc_y, aspect);
                }
            }
            ViewportEvent::DragStart {
                ndc_x,
                ndc_y,
                aspect,
            } => {
                self.drag_last_ndc = Some((ndc_x, ndc_y));
                self.drag_last_bed = self
                    .scene
                    .camera
                    .hit_z0(ndc_x, ndc_y, aspect)
                    .map(|p| (p.x, p.y));
                self.drag_axis = self.scene.gizmo.and_then(|gizmo| {
                    let (origin, dir) = self.scene.camera.ray_from_ndc(ndc_x, ndc_y, aspect);
                    gizmo.pick_axis(origin, dir)
                });
                self.sync_gizmo();
            }
            ViewportEvent::Drag {
                ndc_x,
                ndc_y,
                aspect,
            } => self.apply_tool_drag(ndc_x, ndc_y, aspect),
            ViewportEvent::DragEnd => {
                self.drag_last_ndc = None;
                self.drag_last_bed = None;
                self.drag_axis = None;
                self.sync_gizmo();
            }
            ViewportEvent::CursorMoved {
                ndc_x,
                ndc_y,
                aspect,
                viewport_h,
            } => {
                self.scene.viewport_height = viewport_h.max(1.0);
                self.gizmo_hover = self.cursor_near_selection(ndc_x, ndc_y, aspect);
                self.sync_gizmo();
            }
            ViewportEvent::CursorLeft => {
                self.gizmo_hover = false;
                self.sync_gizmo();
            }
        }
    }

    fn apply_tool_drag(&mut self, ndc_x: f32, ndc_y: f32, aspect: f32) {
        match self.scene.tool {
            PlaterTool::Move => {
                if self.drag_axis == Some(GizmoAxis::Z) {
                    let Some((_, ly)) = self.drag_last_ndc else {
                        self.drag_last_ndc = Some((ndc_x, ndc_y));
                        return;
                    };
                    let dz = (ndc_y - ly) * self.scene.camera.distance * 0.25;
                    if let Some(inst) = self.selected_instance_mut() {
                        inst.offset.z += dz;
                    }
                    self.drag_last_ndc = Some((ndc_x, ndc_y));
                    self.sync_scene_mesh();
                    return;
                }
                let Some(hit) = self.scene.camera.hit_z0(ndc_x, ndc_y, aspect) else {
                    return;
                };
                let Some((lx, ly)) = self.drag_last_bed else {
                    self.drag_last_bed = Some((hit.x, hit.y));
                    return;
                };
                let mut dx = hit.x - lx;
                let mut dy = hit.y - ly;
                match self.drag_axis {
                    Some(GizmoAxis::X) => dy = 0.0,
                    Some(GizmoAxis::Y) => dx = 0.0,
                    Some(GizmoAxis::Z) | None => {}
                }
                if let Some(inst) = self.selected_instance_mut() {
                    inst.offset.x += dx;
                    inst.offset.y += dy;
                }
                self.drag_last_bed = Some((hit.x, hit.y));
                self.sync_scene_mesh();
            }
            PlaterTool::Rotate => {
                let Some((lx, ly)) = self.drag_last_ndc else {
                    self.drag_last_ndc = Some((ndc_x, ndc_y));
                    return;
                };
                let delta = match self.drag_axis {
                    Some(GizmoAxis::Y) => (ndc_y - ly) * 90.0,
                    _ => (ndc_x - lx) * 90.0,
                };
                let axis = self.drag_axis;
                if let Some(inst) = self.selected_instance_mut() {
                    match axis {
                        Some(GizmoAxis::X) => inst.rotation_deg.x += delta,
                        Some(GizmoAxis::Y) => inst.rotation_deg.y += delta,
                        Some(GizmoAxis::Z) | None => inst.rotation_deg.z += delta,
                    }
                }
                self.drag_last_ndc = Some((ndc_x, ndc_y));
                self.sync_scene_mesh();
            }
            PlaterTool::Scale => {
                let Some((lx, _)) = self.drag_last_ndc else {
                    self.drag_last_ndc = Some((ndc_x, ndc_y));
                    return;
                };
                let factor = (1.0 + (ndc_x - lx) * 0.8).clamp(0.5, 1.5);
                let axis = self.drag_axis;
                let uniform = self.uniform_scale && axis.is_none();
                if let Some(inst) = self.selected_instance_mut() {
                    match axis {
                        Some(GizmoAxis::X) => inst.scale.x *= factor,
                        Some(GizmoAxis::Y) => inst.scale.y *= factor,
                        Some(GizmoAxis::Z) => inst.scale.z *= factor,
                        None if uniform => inst.scale *= factor,
                        None => inst.scale.x *= factor,
                    }
                    inst.scale = inst.scale.clamp(Vec3::splat(0.05), Vec3::splat(20.0));
                }
                self.drag_last_ndc = Some((ndc_x, ndc_y));
                self.sync_scene_mesh();
            }
            PlaterTool::Orbit | PlaterTool::LayOnFace => {}
        }
    }

    fn lay_on_face_pick(&mut self, ndc_x: f32, ndc_y: f32, aspect: f32) {
        self.ensure_model();
        let (origin, dir) = self.scene.camera.ray_from_ndc(ndc_x, ndc_y, aspect);
        let idx = self.selected_object;
        let picked = self.model.as_ref().and_then(|model| {
            let obj = model.objects.get(idx)?;
            let mesh = obj.printable_mesh();
            let inst = obj.instances.first().copied().unwrap_or_default();
            let world = inst.apply_to_mesh(&mesh);
            let hit = world.pick_triangle(origin, dir)?;
            let [a, b, c] = world.triangle(world.indices[hit]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            Some((mesh, n))
        });
        let Some((mesh, n)) = picked else {
            self.status = "no triangle under cursor".into();
            return;
        };
        if let Some(inst) = self
            .model
            .as_mut()
            .and_then(|m| m.selected_instance_mut(idx))
        {
            lay_instance_on_normal(&mesh, inst, n);
            self.status = "laid on face".into();
            self.sync_scene_mesh();
        }
    }

    pub(crate) fn arrange_plate(&mut self) {
        self.ensure_model();
        let bed = self.scene.bed.clone();
        let plate = self.plate;
        if let Some(model) = &mut self.model {
            model.arrange_on_plate(plate, &bed, 8.0);
        }
        self.status = "arranged on plate".into();
        self.sync_scene_mesh();
    }

    pub(crate) fn auto_orient_selected(&mut self) {
        self.ensure_model();
        let idx = self.selected_object;
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let Some(obj) = model.objects.get_mut(idx) else {
            return;
        };
        if obj.instances.is_empty() {
            obj.instances.push(Instance::default());
        }
        let mesh = obj.printable_mesh();
        if let Some(inst) = obj.instances.first_mut() {
            auto_orient_instance(&mesh, inst);
        }
        self.status = "auto-oriented".into();
        self.sync_scene_mesh();
    }

    pub(crate) fn mirror_selected(&mut self, axis: u8) {
        self.ensure_model();
        let idx = self.selected_object;
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let Some(obj) = model.objects.get_mut(idx) else {
            return;
        };
        if obj.instances.is_empty() {
            obj.instances.push(Instance::default());
        }
        let mesh = obj.printable_mesh();
        if let Some(inst) = obj.instances.first_mut() {
            match axis {
                0 => inst.scale.x *= -1.0,
                1 => inst.scale.y *= -1.0,
                _ => inst.scale.z *= -1.0,
            }
            drop_instance_to_bed(&mesh, inst);
        }
        self.status = format!("mirrored {}", ["X", "Y", "Z"][axis.min(2) as usize]);
        self.sync_scene_mesh();
    }

    pub(crate) fn rotate_selected_90(&mut self) {
        if let Some(inst) = self.selected_instance_mut() {
            inst.rotation_deg.z += 90.0;
            if inst.rotation_deg.z >= 360.0 {
                inst.rotation_deg.z -= 360.0;
            }
        }
        self.sync_scene_mesh();
    }

    pub(crate) fn drop_selected_to_bed(&mut self) {
        let idx = self.selected_object;
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let Some(obj) = model.objects.get_mut(idx) else {
            return;
        };
        if obj.instances.is_empty() {
            obj.instances.push(Instance::default());
        }
        let mesh = obj.printable_mesh();
        if let Some(inst) = obj.instances.first_mut() {
            drop_instance_to_bed(&mesh, inst);
        }
        self.status = "dropped to bed".into();
        self.sync_scene_mesh();
    }

    pub(crate) fn reset_selected_rotation(&mut self) {
        let rot = self.rotate_open_deg;
        if let Some(inst) = self.selected_instance_mut() {
            inst.rotation_deg = rot;
        }
        self.status = "rotation reset".into();
        self.sync_scene_mesh();
    }

    pub(crate) fn reset_selected_scale(&mut self) {
        if let Some(inst) = self.selected_instance_mut() {
            inst.scale = Vec3::ONE;
        }
        self.status = "scale reset".into();
        self.sync_scene_mesh();
    }

    pub(crate) fn set_coord_space(&mut self, space: CoordSpace) {
        self.coord_space = space;
        self.fill_xform_edits();
    }

    pub(crate) fn set_uniform_scale(&mut self, uniform: bool) {
        self.uniform_scale = uniform;
    }

    pub(crate) fn set_xform_draft(&mut self, field: XformField, text: String) {
        *self.xform_draft_mut(field) = text;
    }

    pub(crate) fn commit_xform_field(&mut self, field: XformField) {
        let text = self.xform_draft(field);
        let Ok(value) = text.trim().parse::<f32>() else {
            self.fill_xform_edits();
            return;
        };
        match field {
            XformField::Pos(axis) => self.set_position_axis(axis as usize, value),
            XformField::RotRel(axis) => self.set_relative_rotation(axis as usize, value),
            XformField::RotAbs(axis) => {
                if let Some(inst) = self.selected_instance_mut() {
                    inst.rotation_deg[axis as usize] = value;
                }
            }
            XformField::ScalePct(axis) => self.set_scale_percent(axis as usize, value),
            XformField::Size(axis) => self.set_size_mm(axis as usize, value),
        }
        self.sync_scene_mesh();
    }

    fn set_position_axis(&mut self, axis: usize, value: f32) {
        let value = value.clamp(-1.0e6, 1.0e6);
        match self.coord_space {
            CoordSpace::World => {
                let center = self
                    .selected_world_aabb()
                    .map(|a| (a.min + a.max) * 0.5)
                    .unwrap_or(Vec3::ZERO);
                if let Some(inst) = self.selected_instance_mut() {
                    inst.offset[axis] += value - center[axis];
                }
            }
            CoordSpace::Object | CoordSpace::Part => {
                if let Some(inst) = self.selected_instance_mut() {
                    inst.offset[axis] = value;
                }
            }
        }
    }

    fn set_relative_rotation(&mut self, axis: usize, value: f32) {
        let new = self.rotate_open_deg[axis] + value;
        if let Some(inst) = self.selected_instance_mut() {
            inst.rotation_deg[axis] = new;
        }
        self.rotate_open_deg[axis] = new;
    }

    fn set_scale_percent(&mut self, axis: usize, percent: f32) {
        let mag = (percent / 100.0).abs().clamp(0.05, 20.0);
        let uniform = self.uniform_scale;
        if let Some(inst) = self.selected_instance_mut() {
            let sign = if inst.scale[axis] < 0.0 { -1.0 } else { 1.0 };
            if uniform {
                let cur = inst.scale[axis].abs().max(1e-6);
                inst.scale *= mag / cur;
            } else {
                inst.scale[axis] = mag * sign;
            }
            inst.scale = inst.scale.clamp(Vec3::splat(-20.0), Vec3::splat(20.0));
            for i in 0..3 {
                if inst.scale[i].abs() < 0.05 {
                    let sign = if inst.scale[i] < 0.0 { -1.0 } else { 1.0 };
                    inst.scale[i] = 0.05 * sign;
                }
            }
        }
    }

    fn set_size_mm(&mut self, axis: usize, size: f32) {
        let size = size.abs().clamp(0.05, 1.0e5);
        let Some(aabb) = self.selected_world_aabb() else {
            return;
        };
        let cur = aabb.size()[axis].abs().max(1e-6);
        let ratio = size / cur;
        let uniform = self.uniform_scale;
        if let Some(inst) = self.selected_instance_mut() {
            if uniform {
                inst.scale *= ratio;
            } else {
                inst.scale[axis] *= ratio;
            }
            inst.scale = inst.scale.clamp(Vec3::splat(-20.0), Vec3::splat(20.0));
        }
    }

    pub(crate) fn selected_instance_mut(&mut self) -> Option<&mut Instance> {
        self.ensure_model();
        let idx = self.selected_object;
        let model = self.model.as_mut()?;
        let obj = model.objects.get_mut(idx)?;
        if obj.instances.is_empty() {
            obj.instances.push(Instance::default());
        }
        obj.instances.first_mut()
    }

    pub(crate) fn selected_world_aabb(&self) -> Option<Aabb3> {
        let obj = self.model.as_ref()?.objects.get(self.selected_object)?;
        let mesh = obj.printable_mesh();
        let inst = obj.instances.first().copied().unwrap_or_default();
        inst.apply_to_mesh(&mesh).aabb()
    }

    pub(crate) fn fill_xform_edits(&mut self) {
        let inst = self
            .model
            .as_ref()
            .and_then(|m| m.selected_instance(self.selected_object))
            .copied()
            .unwrap_or_default();
        let aabb = self.selected_world_aabb();
        let pos = match self.coord_space {
            CoordSpace::World => aabb.map(|a| (a.min + a.max) * 0.5).unwrap_or(inst.offset),
            CoordSpace::Object | CoordSpace::Part => inst.offset,
        };
        self.pos_edit = fmt3(pos);
        self.rot_rel_edit = fmt3(inst.rotation_deg - self.rotate_open_deg);
        self.rot_abs_edit = fmt3(inst.rotation_deg);
        self.scale_pct_edit = [
            format!("{:.2}", inst.scale.x.abs() * 100.0),
            format!("{:.2}", inst.scale.y.abs() * 100.0),
            format!("{:.2}", inst.scale.z.abs() * 100.0),
        ];
        self.size_edit = fmt3(aabb.map(|a| a.size()).unwrap_or(Vec3::ZERO));
    }

    fn xform_draft(&self, field: XformField) -> String {
        match field {
            XformField::Pos(i) => self.pos_edit[i as usize].clone(),
            XformField::RotRel(i) => self.rot_rel_edit[i as usize].clone(),
            XformField::RotAbs(i) => self.rot_abs_edit[i as usize].clone(),
            XformField::ScalePct(i) => self.scale_pct_edit[i as usize].clone(),
            XformField::Size(i) => self.size_edit[i as usize].clone(),
        }
    }

    fn xform_draft_mut(&mut self, field: XformField) -> &mut String {
        match field {
            XformField::Pos(i) => &mut self.pos_edit[i as usize],
            XformField::RotRel(i) => &mut self.rot_rel_edit[i as usize],
            XformField::RotAbs(i) => &mut self.rot_abs_edit[i as usize],
            XformField::ScalePct(i) => &mut self.scale_pct_edit[i as usize],
            XformField::Size(i) => &mut self.size_edit[i as usize],
        }
    }

    pub(crate) fn ensure_model(&mut self) {
        if self.model.is_some() {
            return;
        }
        let mut model = Model::from_mesh("cube", self.scene.mesh.clone());
        model.place_on_bed_if_needed(&self.scene.bed);
        self.model = Some(model);
    }

    pub(crate) fn apply_bed_from_settings(&mut self) {
        let bed: BedShape = self.settings.bed_shape();
        self.scene.set_bed_shape(bed);
    }
}

fn fmt3(v: Vec3) -> [String; 3] {
    [
        format!("{:.2}", v.x),
        format!("{:.2}", v.y),
        format!("{:.2}", v.z),
    ]
}

fn axis_input<'a>(
    label: &'static str,
    draft: &'a str,
    field: XformField,
    unit: &'static str,
) -> Element<'a, Message> {
    row![
        text(label).size(12).width(18),
        text_input(unit, draft)
            .on_input(move |text| Message::XformDraft { field, text })
            .on_submit(Message::XformCommit(field))
            .style(theme::field)
            .width(Fill),
        text(unit).size(12).width(28),
    ]
    .spacing(4)
    .into()
}
