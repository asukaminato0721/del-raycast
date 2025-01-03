pub struct ShapeEntity {
    pub transform_objlcl2world: [f32; 16],
    pub transform_world2objlcl: [f32; 16],
    pub shape: ShapeType,
    pub material_index: Option<usize>,
    pub area_light_index: Option<usize>,
}

pub enum ShapeType {
    TriangleMesh {
        tri2vtx: Vec<usize>,
        vtx2xyz: Vec<f32>,
        vtx2nrm: Vec<f32>,
    },
    Sphere {
        radius: f32,
    },
}

/// # Return
/// - `None`: no hit
/// - `Some((t: f3e2,i_shape_entity: usize, i_elem: usize))`
pub fn intersection_ray_against_shape_entities(
    ray_org: &[f32; 3],
    ray_dir: &[f32; 3],
    shape_entities: &[ShapeEntity],
) -> Option<(f32, usize, usize)> {
    use del_geo_core::mat4_col_major;
    let mut t_min = f32::INFINITY;
    let mut i_shape_entity_min = 0usize;
    let mut i_elem_min = 0usize;
    for (i_shape_entity, shape_entity) in shape_entities.iter().enumerate() {
        let transform_world2objlcl = shape_entity.transform_world2objlcl;
        let ray_org_objlcl =
            mat4_col_major::transform_homogeneous(&transform_world2objlcl, ray_org).unwrap();
        let ray_dir_objlcl = mat4_col_major::transform_direction(&transform_world2objlcl, ray_dir);
        match &shape_entity.shape {
            ShapeType::TriangleMesh {
                tri2vtx, vtx2xyz, ..
            } => {
                if let Some((t, i_tri)) =
                    del_msh_core::trimesh3_search_bruteforce::first_intersection_ray(
                        &ray_org_objlcl,
                        &ray_dir_objlcl,
                        tri2vtx,
                        vtx2xyz,
                    )
                {
                    if t < t_min && t > 0f32 {
                        t_min = t;
                        i_shape_entity_min = i_shape_entity;
                        i_elem_min = i_tri;
                    }
                }
            }
            ShapeType::Sphere { radius } => {
                if let Some(t) = del_geo_core::sphere::intersection_ray::<f32>(
                    *radius,
                    &[0f32; 3],
                    &ray_org_objlcl,
                    &ray_dir_objlcl,
                ) {
                    if t < t_min && t > 0f32 {
                        t_min = t;
                        i_shape_entity_min = i_shape_entity;
                        i_elem_min = 0usize;
                    }
                }
            }
        };
    }
    if t_min == f32::INFINITY {
        None
    } else {
        Some((t_min, i_shape_entity_min, i_elem_min))
    }
}

pub fn normal_at(se: &ShapeEntity, hit_pos_world: &[f32; 3], i_elem: usize) -> [f32; 3] {
    use del_geo_core::mat4_col_major;
    let hit_pos_objlcl =
        mat4_col_major::transform_homogeneous(&se.transform_world2objlcl, hit_pos_world).unwrap();
    use crate::shape::ShapeType;
    let hit_nrm_objlcl = match &se.shape {
        ShapeType::TriangleMesh {
            tri2vtx, vtx2xyz, ..
        } => del_msh_core::trimesh3::to_tri3(tri2vtx, vtx2xyz, i_elem).normal(),
        ShapeType::Sphere { radius: _ } => hit_pos_objlcl.to_owned(),
    };
    let hit_nrm_world =
        mat4_col_major::transform_direction(&se.transform_objlcl2world, &hit_nrm_objlcl);
    del_geo_core::vec3::normalize(&hit_nrm_world)
}

pub fn write_wavefront_obj_file_from_camera_view(
    file_name: &str,
    shape_entities: &[ShapeEntity],
    transform_world2camlcl: &[f32; 16],
) -> anyhow::Result<()> {
    // make obj file
    let mut tri2vtx_out: Vec<usize> = vec![];
    let mut vtx2xyz_out: Vec<f32> = vec![];
    for shape_entity in shape_entities.iter() {
        let (tri2vtx, vtx2xyz_camlcl) = {
            use crate::shape::ShapeType;
            let (tri2vtx, vtx2xyz_objlcl) = match &shape_entity.shape {
                ShapeType::TriangleMesh {
                    tri2vtx, vtx2xyz, ..
                } => (tri2vtx.to_owned(), vtx2xyz.to_owned()),
                ShapeType::Sphere { radius } => {
                    let (tri2vtx, vtx2xyz) =
                        del_msh_core::trimesh3_primitive::sphere_yup::<usize, f32>(*radius, 32, 32);
                    (tri2vtx, vtx2xyz)
                }
            };
            let vtx2xyz_world = del_msh_core::vtx2xyz::transform(
                &vtx2xyz_objlcl,
                &shape_entity.transform_objlcl2world,
            );
            let vtx2xyz_camlcl =
                del_msh_core::vtx2xyz::transform(&vtx2xyz_world, transform_world2camlcl);
            (tri2vtx, vtx2xyz_camlcl)
        };
        del_msh_core::uniform_mesh::merge(
            &mut tri2vtx_out,
            &mut vtx2xyz_out,
            &tri2vtx,
            &vtx2xyz_camlcl,
            3,
        );
    }
    del_msh_core::io_obj::save_tri2vtx_vtx2xyz(file_name, &tri2vtx_out, &vtx2xyz_out, 3)?;
    Ok(())
}

pub fn triangle_mesh_normal_at(
    tri2vtx: &[usize],
    vtx2xyz: &[f32],
    vtx2nrm: &[f32],
    pos: &[f32; 3],
    i_tri: usize,
) -> [f32; 3] {
    assert!(i_tri < tri2vtx.len() / 3);
    let iv0 = tri2vtx[i_tri * 3];
    let iv1 = tri2vtx[i_tri * 3 + 1];
    let iv2 = tri2vtx[i_tri * 3 + 2];
    let p0 = arrayref::array_ref![vtx2xyz, iv0 * 3, 3];
    let p1 = arrayref::array_ref![vtx2xyz, iv1 * 3, 3];
    let p2 = arrayref::array_ref![vtx2xyz, iv2 * 3, 3];
    let bc = del_geo_core::tri3::to_barycentric_coords(p0, p1, p2, pos);
    let n0 = arrayref::array_ref![vtx2nrm, iv0 * 3, 3];
    let n1 = arrayref::array_ref![vtx2nrm, iv1 * 3, 3];
    let n2 = arrayref::array_ref![vtx2nrm, iv2 * 3, 3];
    let n = [
        bc[0] * n0[0] + bc[1] * n1[0] + bc[2] * n2[0],
        bc[0] * n0[1] + bc[1] * n1[1] + bc[2] * n2[1],
        bc[0] * n0[2] + bc[1] * n1[2] + bc[2] * n2[2],
    ];
    del_geo_core::vec3::normalize(&n)
}
