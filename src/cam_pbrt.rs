pub fn cast_ray(
    ix: usize,
    iy: usize,
    img_shape: (usize, usize),
    fov: f32,
    transform_cam_lcl2glbl: [f32; 16]) -> ([f32; 3], [f32; 3])
{
    assert!(ix < img_shape.0 && iy < img_shape.1);
    let focal_dis = 0.5 / (fov / 2.0).to_radians().tan();
    let (screen_width, screen_height) = if img_shape.0 >  img_shape.1 {
        (img_shape.0 as f32 / img_shape.1 as f32, 1f32)
    } else{
        (1f32, img_shape.1 as f32 / img_shape.0 as f32)
    };
    let x = ((ix as f32 + 0.5) / img_shape.0 as f32 - 0.5) * screen_width;
    let y = (0.5 - (iy as f32 + 0.5) / img_shape.1 as f32) *screen_height;
    let z = focal_dis;
    let mut dir = [x, y, z];
    let mut org = [0.0, 0.0, 0.0];
    use del_geo_core::mat4_col_major;
    dir = mat4_col_major::transform_vector(&transform_cam_lcl2glbl, &dir);
    org = mat4_col_major::transform_homogeneous(&transform_cam_lcl2glbl, &org).unwrap();
    (org, dir)
}