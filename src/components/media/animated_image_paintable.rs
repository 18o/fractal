#[cfg(target_os = "macos")]
use crate::utils::media::image::{Frame, Image, ImageData};
#[cfg(target_os = "linux")]
use glycin::{Frame, Image};
use gtk::{gdk, glib, glib::clone, graphene, prelude::*, subclass::prelude::*};
use std::sync::Arc;
use tracing::error;

use crate::{
    spawn, spawn_tokio,
    utils::{CountedRef, File},
};

mod imp {
    use std::{
        cell::{OnceCell, RefCell},
        sync::Arc,
    };

    use super::*;

    #[derive(Default)]
    pub struct AnimatedImagePaintable {
        /// The image loader.
        #[cfg(target_os = "linux")]
        image_loader: OnceCell<Arc<Image<'static>>>,
        #[cfg(target_os = "macos")]
        image_data: OnceCell<Arc<ImageData>>,
        /// The file of the image.
        file: OnceCell<File>,
        /// The current frame that is displayed.
        pub(super) current_frame: RefCell<Option<Arc<Frame>>>,
        /// The next frame of the animation, if any.
        next_frame: RefCell<Option<Arc<Frame>>>,
        /// The source ID of the timeout to load the next frame, if any.
        timeout_source_id: RefCell<Option<glib::SourceId>>,
        /// The counted reference for the animation.
        ///
        /// When the count is 0, the animation is paused.
        animation_ref: OnceCell<CountedRef>,
        /// Current frame index for macOS animation
        #[cfg(target_os = "macos")]
        current_frame_index: RefCell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AnimatedImagePaintable {
        const NAME: &'static str = "AnimatedImagePaintable";
        type Type = super::AnimatedImagePaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for AnimatedImagePaintable {}

    impl PaintableImpl for AnimatedImagePaintable {
        fn intrinsic_height(&self) -> i32 {
            #[cfg(target_os = "linux")]
            {
                self.current_frame
                    .borrow()
                    .as_ref()
                    .map_or_else(|| self.image_loader().info().height, |f| f.height())
                    .try_into()
                    .unwrap_or(i32::MAX)
            }

            #[cfg(target_os = "macos")]
            {
                self.current_frame
                    .borrow()
                    .as_ref()
                    .and_then(|f| f.dimensions())
                    .map_or(100, |d| d.height as i32)
            }
        }

        fn intrinsic_width(&self) -> i32 {
            #[cfg(target_os = "linux")]
            {
                self.current_frame
                    .borrow()
                    .as_ref()
                    .map_or_else(|| self.image_loader().info().width, |f| f.width())
                    .try_into()
                    .unwrap_or(i32::MAX)
            }

            #[cfg(target_os = "macos")]
            {
                self.current_frame
                    .borrow()
                    .as_ref()
                    .and_then(|f| f.dimensions())
                    .map_or(100, |d| d.width as i32) // 默认宽度
            }
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if let Some(frame) = &*self.current_frame.borrow() {
                frame.texture().snapshot(snapshot, width, height);
            } else {
                let snapshot = snapshot.downcast_ref::<gtk::Snapshot>().unwrap();
                snapshot.append_color(
                    &gdk::RGBA::BLACK,
                    &graphene::Rect::new(0., 0., width as f32, height as f32),
                );
            }
        }

        fn flags(&self) -> gdk::PaintableFlags {
            gdk::PaintableFlags::SIZE
        }

        fn current_image(&self) -> gdk::Paintable {
            let snapshot = gtk::Snapshot::new();
            self.snapshot(
                snapshot.upcast_ref(),
                self.intrinsic_width().into(),
                self.intrinsic_height().into(),
            );

            snapshot
                .to_paintable(None)
                .expect("snapshot should always work")
        }
    }

    impl AnimatedImagePaintable {
        /// The image loader.
        #[cfg(target_os = "linux")]
        fn image_loader(&self) -> &Arc<Image<'static>> {
            self.image_loader
                .get()
                .expect("image loader is initialized")
        }

        /// Initialize the image.
        #[cfg(target_os = "linux")]
        pub(super) fn init(
            &self,
            file: File,
            image_loader: Arc<Image<'static>>,
            first_frame: Arc<Frame>,
        ) {
            self.file.set(file).expect("file is uninitialized");
            self.image_loader
                .set(image_loader)
                .expect("image loader is uninitialized");
            self.current_frame.replace(Some(first_frame));

            self.update_animation();
        }

        #[cfg(target_os = "macos")]
        pub(super) fn init(&self, file: File, image_data: Arc<ImageData>, first_frame: Arc<Frame>) {
            self.file.set(file).expect("file is uninitialized");
            self.image_data
                .set(image_data)
                .expect("image data is uninitialized");
            self.current_frame.replace(Some(first_frame));
            self.current_frame_index.replace(0);

            self.update_animation();
        }

        /// Show the next frame of the animation.
        fn show_next_frame(&self) {
            // Drop the timeout source ID so we know we are not waiting for it.
            self.timeout_source_id.take();

            #[cfg(target_os = "linux")]
            {
                let Some(next_frame) = self.next_frame.take() else {
                    // Wait for the next frame to be loaded.
                    return;
                };

                self.current_frame.replace(Some(next_frame));
            }

            #[cfg(target_os = "macos")]
            {
                if let Some(image_data) = self.image_data.get() {
                    if let ImageData::Animated { frames, .. } = image_data.as_ref() {
                        let mut index = self.current_frame_index.borrow_mut();
                        *index = (*index + 1) % frames.len();

                        if let Some(frame_data) = frames.get(*index) {
                            let mut frame = Frame::new_from_dynamic_image(frame_data.image.clone());
                            if let Frame::Image { delay, .. } = &mut frame {
                                *delay = frame_data.delay;
                            }
                            self.current_frame.replace(Some(Arc::new(frame)));
                        }
                    }
                }
            }

            // Invalidate the contents so that the new frame will be rendered.
            self.obj().invalidate_contents();

            self.update_animation();
        }

        /// The counted reference of the animation.
        pub(super) fn animation_ref(&self) -> &CountedRef {
            self.animation_ref.get_or_init(|| {
                CountedRef::new(
                    clone!(
                        #[weak(rename_to = imp)]
                        self,
                        move || {
                            imp.update_animation();
                        }
                    ),
                    clone!(
                        #[weak(rename_to = imp)]
                        self,
                        move || {
                            imp.update_animation();
                        }
                    ),
                )
            })
        }

        /// Prepare the next frame of the animation or stop the animation,
        /// depending on the refcount.
        fn update_animation(&self) {
            if self.animation_ref().count() == 0 {
                // We should not animate, remove the timeout if it exists.
                if let Some(source) = self.timeout_source_id.take() {
                    source.remove();
                }
                return;
            } else if self.timeout_source_id.borrow().is_some() {
                // We are already waiting for the next update.
                return;
            }

            let Some(delay) = self.current_frame.borrow().as_ref().and_then(|f| f.delay()) else {
                return;
            };

            // Set the timeout to update the animation.
            let source_id = glib::timeout_add_local_once(
                delay,
                clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move || {
                        imp.show_next_frame();
                    }
                ),
            );
            self.timeout_source_id.replace(Some(source_id));

            #[cfg(target_os = "linux")]
            {
                spawn!(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    async move {
                        imp.load_next_frame_inner().await;
                    }
                ));
            }
        }
        #[cfg(target_os = "linux")]
        async fn load_next_frame_inner(&self) {
            let image = self.image_loader().clone();

            let result = spawn_tokio!(async move { image.next_frame().await })
                .await
                .unwrap();

            match result {
                Ok(next_frame) => {
                    self.next_frame.replace(Some(next_frame.into()));

                    // In case loading the frame took longer than the delay between frames.
                    if self.timeout_source_id.borrow().is_none() {
                        self.show_next_frame();
                    }
                }
                Err(error) => {
                    error!("Failed to load next frame: {error}");
                    // Do nothing, the animation will stop.
                }
            }
        }
    }
}

glib::wrapper! {
    /// A paintable to display an animated image.
    pub struct AnimatedImagePaintable(ObjectSubclass<imp::AnimatedImagePaintable>)
        @implements gdk::Paintable;
}

impl AnimatedImagePaintable {
    /// Construct an `AnimatedImagePaintable` with the given loader and first
    /// frame.
    #[cfg(target_os = "linux")]
    pub(crate) fn new(
        file: File,
        image_loader: Arc<Image<'static>>,
        first_frame: Arc<Frame>,
    ) -> Self {
        let obj = glib::Object::new::<Self>();

        obj.imp().init(file, image_loader, first_frame);

        obj
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn new(file: File, image_data: Arc<ImageData>, first_frame: Arc<Frame>) -> Self {
        let obj = glib::Object::new::<Self>();
        obj.imp().init(file, image_data, first_frame);
        obj
    }

    /// Get the current `GdkTexture` of this paintable, if any.
    pub(crate) fn current_texture(&self) -> Option<gdk::Texture> {
        Some(self.imp().current_frame.borrow().as_ref()?.texture())
    }

    /// Get an animation ref.
    pub(crate) fn animation_ref(&self) -> CountedRef {
        self.imp().animation_ref().clone()
    }
}
