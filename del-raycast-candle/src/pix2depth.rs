#[cfg(feature = "cuda")]
use candle_core::CudaStorage;
use candle_core::{CpuStorage, Device, Layout, Shape, Storage, Tensor};
use std::ops::Deref;

pub fn hoge(
    tri2vtx: &[u32],
    vtx2xyz: &[f32],
    (width, height): (usize, usize),
    pix2tri: &[u32],
    dw_pix2depth: &[f32],
    transform_ndc2world: &[f32; 16],
) -> Vec<f32> {
    let num_vtx = vtx2xyz.len() / 3;
    let mut dw_vtx2xyz = vec![0f32; num_vtx * 3];
    for i_h in 0..height {
        for i_w in 0..width {
            let i_tri = pix2tri[i_h * width + i_w];
            if i_tri == u32::MAX {
                continue;
            }
            let (ray_org, ray_dir) = del_raycast_core::cam3::ray3_homogeneous(
                (i_w, i_h),
                (width, height),
                transform_ndc2world,
            );
            let i_tri = i_tri as usize;
            let (p0, p1, p2) = del_msh_core::trimesh3::to_corner_points(tri2vtx, vtx2xyz, i_tri);
            let Some((_t, _u, _v, data)) =
                del_geo_core::tri3::ray_triangle_intersection(&ray_org, &ray_dir, &p0, &p1, &p2)
            else {
                continue;
            };
            let dw_depth = dw_pix2depth[i_h * width + i_w];
            let (dw_p0, dw_p1, dw_p2) =
                del_geo_core::tri3::dldw_ray_triangle_intersection_(-dw_depth, 0., 0., &data);
            use del_geo_core::vec3::Vec3;
            let scale = data.dir.norm();
            let dw_p0 = dw_p0.scale(scale);
            let dw_p1 = dw_p1.scale(scale);
            let dw_p2 = dw_p2.scale(scale);
            let iv0 = tri2vtx[i_tri * 3] as usize;
            let iv1 = tri2vtx[i_tri * 3 + 1] as usize;
            let iv2 = tri2vtx[i_tri * 3 + 2] as usize;
            arrayref::array_mut_ref![dw_vtx2xyz, iv0 * 3, 3].add_in_place(&dw_p0);
            arrayref::array_mut_ref![dw_vtx2xyz, iv1 * 3, 3].add_in_place(&dw_p1);
            arrayref::array_mut_ref![dw_vtx2xyz, iv2 * 3, 3].add_in_place(&dw_p2);
        }
    }
    dw_vtx2xyz
}

pub struct Pix2Depth {
    pub tri2vtx: Tensor,
    pub pix2tri: Tensor,
    pub transform_ndc2world: Tensor, // transform column major
}

impl candle_core::CustomOp1 for Pix2Depth {
    fn name(&self) -> &'static str {
        "pix2depth"
    }

    fn cpu_fwd(
        &self,
        vtx2xyz: &CpuStorage,
        l_vtx2xyz: &Layout,
    ) -> candle_core::Result<(CpuStorage, Shape)> {
        let (_num_vtx, three) = l_vtx2xyz.shape().dims2()?;
        assert_eq!(three, 3);
        let vtx2xyz = vtx2xyz.as_slice::<f32>()?;
        get_cpu_slice_from_tensor!(tri2vtx, storage, self.tri2vtx, u32);
        get_cpu_slice_from_tensor!(pix2tri, storage, self.pix2tri, u32);
        get_cpu_slice_from_tensor!(transform_ndc2world, storage, self.transform_ndc2world, f32);
        let transform_ndc2world = arrayref::array_ref![transform_ndc2world, 0, 16];
        let transform_world2ndc =
            del_geo_core::mat4_col_major::try_inverse(transform_ndc2world).unwrap();
        //
        let img_shape = (self.pix2tri.dim(0)?, self.pix2tri.dim(1)?);
        let fn_pix2depth = |i_pix: usize| -> Option<f32> {
            let (i_w, i_h) = (i_pix % img_shape.0, i_pix / img_shape.0);
            let i_tri = pix2tri[i_h * img_shape.0 + i_w];
            if i_tri == u32::MAX {
                return None;
            }
            let (ray_org, ray_dir) = del_raycast_core::cam3::ray3_homogeneous(
                (i_w, i_h),
                img_shape,
                transform_ndc2world,
            );
            let tri = del_msh_core::trimesh3::to_tri3(tri2vtx, vtx2xyz, i_tri as usize);
            let coeff = del_geo_core::tri3::intersection_against_line(
                tri.p0, tri.p1, tri.p2, &ray_org, &ray_dir,
            )
            .unwrap();
            let pos_world = del_geo_core::vec3::axpy(coeff, &ray_dir, &ray_org);
            let pos_ndc = del_geo_core::mat4_col_major::transform_homogeneous(
                &transform_world2ndc,
                &pos_world,
            )
            .unwrap();
            let depth_ndc = (pos_ndc[2] + 1f32) * 0.5f32;
            Some(depth_ndc)
        };
        let mut pix2depth = vec![0f32; img_shape.0 * img_shape.1];
        use rayon::prelude::*;
        pix2depth
            .par_iter_mut()
            .enumerate()
            .for_each(|(i_pix, depth)| {
                *depth = fn_pix2depth(i_pix).unwrap_or(0f32);
            });
        let shape = candle_core::Shape::from((img_shape.1, img_shape.0));
        let storage = candle_core::WithDType::to_cpu_storage_owned(pix2depth);
        Ok((storage, shape))
    }

    #[cfg(feature = "cuda")]
    fn cuda_fwd(
        &self,
        vtx2xyz: &CudaStorage,
        l_vtx2xyz: &Layout,
    ) -> candle_core::Result<(CudaStorage, Shape)> {
        use candle_core::cuda_backend::CudaStorage;
        use candle_core::cuda_backend::WrapErr;
        assert_eq!(l_vtx2xyz.dim(1)?, 3);
        let img_shape = (self.pix2tri.dim(0)?, self.pix2tri.dim(1)?);
        //get_cuda_slice_from_tensor!(vtx2xyz, device_vtx2xyz, vtx2xyz);
        let device = &vtx2xyz.device;
        let vtx2xyz = vtx2xyz.as_cuda_slice::<f32>()?;
        get_cuda_slice_from_tensor!(pix2tri, storage_pix2tri, layout_pix2tri, self.pix2tri, u32);
        get_cuda_slice_from_tensor!(tri2vtx, storage_tri2vtx, _layout_tri2vtx, self.tri2vtx, u32);
        get_cuda_slice_from_tensor!(
            transform_ndc2world,
            storage_transform_ndc2world,
            _layout_transform_ndc2world,
            self.transform_ndc2world,
            f32
        );
        let mut pix2depth = unsafe { device.alloc::<f32>(img_shape.0 * img_shape.1) }.w()?;
        del_raycast_cudarc::pix2depth::pix2depth(
            device,
            img_shape,
            &mut pix2depth,
            pix2tri,
            tri2vtx,
            vtx2xyz,
            transform_ndc2world,
        )
        .w()?;
        let pix2depth = CudaStorage::wrap_cuda_slice(pix2depth, device.clone());
        Ok((pix2depth, layout_pix2tri.shape().clone()))
    }

    /// This function takes as argument the argument `arg` used in the forward pass, the result
    /// produced by the forward operation `res` and the gradient of the result `grad_res`.
    /// The function should return the gradient of the argument.
    #[allow(clippy::identity_op)]
    fn bwd(
        &self,
        vtx2xyz: &Tensor,
        pix2depth: &Tensor,
        dw_pix2depth: &Tensor,
    ) -> candle_core::Result<Option<Tensor>> {
        match vtx2xyz.device() {
            Device::Cpu => {
                let (num_vtx, three) = vtx2xyz.shape().dims2()?;
                assert_eq!(three, 3);
                assert_eq!(pix2depth.shape(), dw_pix2depth.shape());
                let (height, width) = pix2depth.shape().dims2()?;
                get_cpu_slice_from_tensor!(tri2vtx, storage, self.tri2vtx, u32);
                get_cpu_slice_from_tensor!(vtx2xyz, storage, vtx2xyz, f32);
                get_cpu_slice_from_tensor!(pix2tri, storage, self.pix2tri, u32);
                get_cpu_slice_from_tensor!(
                    transform_ndc2world,
                    storage,
                    self.transform_ndc2world,
                    f32
                );
                let transform_ndc2world = arrayref::array_ref![transform_ndc2world, 0, 16];
                get_cpu_slice_from_tensor!(dw_pix2depth, storage, dw_pix2depth, f32);
                let dw_vtx2xyz = hoge(
                    tri2vtx,
                    vtx2xyz,
                    (width, height),
                    pix2tri,
                    dw_pix2depth,
                    transform_ndc2world,
                );
                //
                let dw_vtx2xyz = Tensor::from_vec(
                    dw_vtx2xyz,
                    candle_core::Shape::from((num_vtx, 3)),
                    &Device::Cpu,
                )?;
                Ok(Some(dw_vtx2xyz))
            }
            Device::Cuda(_cuda_device) => {
                todo!()
            }
            _ => panic!(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use candle_core::CudaStorage;
    use candle_core::{Device, Tensor};

    fn render(
        device: &Device,
        tri2vtx: &Tensor,
        vtx2xyz: &Tensor,
        img_shape: (usize, usize),
        transform_ndc2world: &Tensor,
    ) -> candle_core::Result<Tensor> {
        let tri2vtx = tri2vtx.to_device(&device)?;
        let vtx2xyz = vtx2xyz.to_device(&device)?;
        let transform_ndc2world = transform_ndc2world.to_device(&device)?;
        let bvhdata =
            del_msh_candle::bvhnode2aabb::BvhForTriMesh::from_trimesh(&tri2vtx, &vtx2xyz)?;
        let pix2tri = crate::pix2tri::from_trimesh3(
            &tri2vtx,
            &vtx2xyz,
            &bvhdata.bvhnodes,
            &bvhdata.bvhnode2aabb,
            img_shape,
            &transform_ndc2world,
        )?;
        let render = crate::pix2depth::Pix2Depth {
            tri2vtx: tri2vtx.clone(),
            pix2tri: pix2tri.clone(),
            transform_ndc2world: transform_ndc2world.clone(),
        };
        Ok(vtx2xyz.apply_op1(render)?)
    }

    #[test]
    fn test_optimize_depth() -> anyhow::Result<()> {
        let (tri2vtx, vtx2xyz) =
            del_msh_core::trimesh3_primitive::sphere_yup::<u32, f32>(0.8, 32, 32);
        let vtx2xyz = {
            let mut vtx2xyz_new = vtx2xyz.clone();
            del_msh_core::vtx2xyz::translate_then_scale(
                &mut vtx2xyz_new,
                &vtx2xyz,
                &[0.2, 0.0, 0.0],
                1.0,
            );
            vtx2xyz_new
        };
        let num_tri = tri2vtx.len() / 3;
        let tri2vtx = Tensor::from_vec(tri2vtx, (num_tri, 3), &candle_core::Device::Cpu)?;
        let num_vtx = vtx2xyz.len() / 3;
        let vtx2xyz = candle_core::Var::from_vec(vtx2xyz, (num_vtx, 3), &candle_core::Device::Cpu)?;
        let img_shape = (200, 200);
        //
        let transform_ndc2world = del_geo_core::mat4_col_major::from_identity::<f32>();
        let (pix2depth_trg, pix2mask) = {
            let mut img2depth_trg = vec![0f32; img_shape.0 * img_shape.1];
            let mut img2mask = vec![0f32; img_shape.0 * img_shape.1];
            for i_h in 0..img_shape.1 {
                for i_w in 0..img_shape.0 {
                    let (ray_org, _ray_dir) = del_raycast_core::cam3::ray3_homogeneous(
                        (i_w, i_h),
                        img_shape,
                        &transform_ndc2world,
                    );
                    let x = ray_org[0];
                    let y = ray_org[1];
                    let r = (x * x + y * y).sqrt();
                    if r > 0.5 {
                        continue;
                    }
                    img2depth_trg[i_h * img_shape.0 + i_w] = 0.6;
                    img2mask[i_h * img_shape.0 + i_w] = 1.0;
                }
            }
            let img2depth_trg = Tensor::from_vec(img2depth_trg, img_shape, &Device::Cpu)?;
            let img2mask = Tensor::from_vec(img2mask, img_shape, &Device::Cpu)?;
            (img2depth_trg, img2mask)
        };
        let transform_ndc2world = Tensor::from_vec(transform_ndc2world.to_vec(), 16, &Device::Cpu)?;
        {
            // output target images
            let pix2depth_trg = pix2depth_trg.flatten_all()?.to_vec1::<f32>()?;
            del_canvas::write_png_from_float_image_grayscale(
                "../target/pix2depth_trg.png",
                img_shape,
                &pix2depth_trg,
            )?;
            //
            let pix2mask = pix2mask.flatten_all()?.to_vec1::<f32>()?;
            del_canvas::write_png_from_float_image_grayscale(
                "../target/pix2mask.png",
                img_shape,
                &pix2mask,
            )?;
        }
        #[cfg(feature = "cuda")]
        {
            let conj = Tensor::rand(0f32, 1f32, img_shape, &Device::Cpu)?;
            // try gpu depth render
            let pix2depth_cpu = render(
                &Device::Cpu,
                &tri2vtx,
                &vtx2xyz,
                img_shape,
                &transform_ndc2world,
            )?;
            let loss_cpu = pix2depth_cpu.mul(&conj)?.sum_all()?;
            dbg!(loss_cpu.to_vec0::<f32>()?);
            let grad_vtx2xyz_cpu = loss_cpu.backward()?.get(&vtx2xyz).unwrap().to_owned();
            let pix2depth_cpu = pix2depth_cpu.flatten_all()?.to_vec1::<f32>()?;
            //
            let device = Device::new_cuda(0)?;
            let conj_cuda = conj.to_device(&device)?;
            let pix2depth_cuda =
                render(&device, &tri2vtx, &vtx2xyz, img_shape, &transform_ndc2world)?;
            let loss_cuda = pix2depth_cuda.mul(&conj_cuda)?.sum_all()?;
            dbg!(loss_cuda.to_vec0::<f32>()?);
            // let grad_vtx2xyz_cuda = loss_cuda.backward()?.get(&vtx2xyz).unwrap().to_owned();
            let pix2depth_cuda = pix2depth_cuda.flatten_all()?.to_vec1::<f32>()?;
            pix2depth_cpu
                .iter()
                .zip(pix2depth_cuda.iter())
                .for_each(|(a, b)| {
                    assert!((a - b).abs() < 1.0e-6);
                });
        }

        let mut optimizer = crate::gd_with_laplacian_reparam::Optimizer::new(
            vtx2xyz.clone(),
            0.001,
            tri2vtx.clone(),
            vtx2xyz.dims2()?.0,
            0.8,
        )?;

        // let mut optimizer = candle_nn::AdamW::new_lr(vec!(vtx2xyz.clone()), 0.01)?;

        for itr in 0..100 {
            let bvhdata =
                del_msh_candle::bvhnode2aabb::BvhForTriMesh::from_trimesh(&tri2vtx, &vtx2xyz)?;
            let pix2tri = crate::pix2tri::from_trimesh3(
                &tri2vtx,
                &vtx2xyz,
                &bvhdata.bvhnodes,
                &bvhdata.bvhnode2aabb,
                img_shape,
                &transform_ndc2world,
            )?;
            let render = crate::pix2depth::Pix2Depth {
                tri2vtx: tri2vtx.clone(),
                pix2tri: pix2tri.clone(),
                transform_ndc2world: transform_ndc2world.clone(),
            };
            let pix2depth = vtx2xyz.apply_op1(render)?;
            dbg!(pix2depth.shape());
            let pix2diff = pix2depth.sub(&pix2depth_trg)?.mul(&pix2mask)?;
            {
                let pix2depth = pix2depth.flatten_all()?.to_vec1::<f32>()?;
                del_canvas::write_png_from_float_image_grayscale(
                    "../target/pix2depth.png",
                    img_shape,
                    &pix2depth,
                )?;
                let pix2diff = (pix2diff.clone() * 10.0)?
                    .abs()?
                    .flatten_all()?
                    .to_vec1::<f32>()?;
                del_canvas::write_png_from_float_image_grayscale(
                    "../target/pix2diff.png",
                    img_shape,
                    &pix2diff,
                )?;
            }
            let loss = pix2diff.sqr()?.sum_all()?;
            println!("loss: {}", loss.to_vec0::<f32>()?);
            optimizer.step(&loss.backward()?)?;
            {
                let vtx2xyz = vtx2xyz.flatten_all()?.to_vec1::<f32>()?;
                let tri2vtx = tri2vtx.flatten_all()?.to_vec1::<u32>()?;
                del_msh_core::io_obj::save_tri2vtx_vtx2xyz(
                    format!("../target/hoge_{}.obj", itr),
                    &tri2vtx,
                    &vtx2xyz,
                    3,
                )?;
            }
        }
        Ok(())
    }
}