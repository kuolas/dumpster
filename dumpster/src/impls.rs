/*
   dumpster, a cycle-tracking garbage collector for Rust.
   Copyright (C) 2023 Clayton Ramsey.

   This program is free software: you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation, either version 3 of the License, or
   (at your option) any later version.

   This program is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

//! Implementations of [`Collectable`] for common data types.

use std::{
    cell::RefCell,
    collections::{BinaryHeap, HashSet, LinkedList, VecDeque},
    sync::atomic::AtomicUsize,
};

use super::{Collectable, RefGraph};

unsafe impl<'a, T> Collectable for &'a T {
    #[inline]
    fn add_to_ref_graph(&self, _: &mut RefGraph) {}
    #[inline]
    fn sweep(&self, _: bool, _: &mut RefGraph) {}
    #[inline]
    unsafe fn destroy_gcs(&mut self, _: &RefGraph) {}
}

unsafe impl<T: Collectable + ?Sized> Collectable for RefCell<T> {
    #[inline]
    fn add_to_ref_graph(&self, ref_graph: &mut RefGraph) {
        self.borrow().add_to_ref_graph(ref_graph);
    }

    #[inline]
    fn sweep(&self, is_accessible: bool, ref_graph: &mut RefGraph) {
        self.borrow().sweep(is_accessible, ref_graph);
    }

    #[inline]
    unsafe fn destroy_gcs(&mut self, ref_graph: &RefGraph) {
        self.borrow_mut().destroy_gcs(ref_graph);
    }
}

unsafe impl<T: Collectable> Collectable for Option<T> {
    #[inline]
    fn add_to_ref_graph(&self, ref_graph: &mut RefGraph) {
        if let Some(v) = self {
            v.add_to_ref_graph(ref_graph);
        }
    }

    #[inline]
    fn sweep(&self, is_accessible: bool, ref_graph: &mut RefGraph) {
        if let Some(v) = self {
            v.sweep(is_accessible, ref_graph);
        }
    }

    #[inline]
    unsafe fn destroy_gcs(&mut self, ref_graph: &RefGraph) {
        if let Some(x) = self.as_mut() {
            x.destroy_gcs(ref_graph);
        }
    }
}

/// Implement [`Collectable`] for a collection data structure which has some method `iter()` that
/// iterates over all elements of the data structure and `iter_mut()` which does the same over
/// mutable references.
macro_rules! collectable_collection_impl {
    ($x: ty) => {
        unsafe impl<T: Collectable> Collectable for $x {
            #[inline]
            fn add_to_ref_graph(&self, ref_graph: &mut RefGraph) {
                self.iter()
                    .for_each(|elem| elem.add_to_ref_graph(ref_graph));
            }

            #[inline]
            fn sweep(&self, is_accessible: bool, ref_graph: &mut RefGraph) {
                self.iter()
                    .for_each(|elem| elem.sweep(is_accessible, ref_graph));
            }

            #[inline]
            unsafe fn destroy_gcs(&mut self, ref_graph: &RefGraph) {
                self.iter_mut().for_each(|x| x.destroy_gcs(ref_graph));
            }
        }
    };
}

collectable_collection_impl!(Vec<T>);
collectable_collection_impl!(VecDeque<T>);
collectable_collection_impl!(LinkedList<T>);

/// Implement [`Collectable`] for a set-like data structure which freezes its elements.
macro_rules! collectable_set_impl {
    ($x: ty) => {
        unsafe impl<T: Collectable> Collectable for $x {
            #[inline]
            fn add_to_ref_graph(&self, ref_graph: &mut RefGraph) {
                self.iter()
                    .for_each(|elem| elem.add_to_ref_graph(ref_graph));
            }

            #[inline]
            fn sweep(&self, is_accessible: bool, ref_graph: &mut RefGraph) {
                self.iter()
                    .for_each(|elem| elem.sweep(is_accessible, ref_graph));
            }

            #[inline]
            unsafe fn destroy_gcs(&mut self, ref_graph: &RefGraph) {
                self.drain().for_each(|mut x| x.destroy_gcs(ref_graph));
            }
        }
    };
}

collectable_set_impl!(HashSet<T>);
collectable_set_impl!(BinaryHeap<T>);
// collectable_set_impl!(BTreeSet<T>); // awaiting stabilization of `drain` on `BTreeSet`

/// Implement [`Collectable`] for a trivially-collected type which contains no  [`Gc`]s in its
/// fields.
macro_rules! collectable_trivial_impl {
    ($x: ty) => {
        unsafe impl Collectable for $x {
            #[inline]
            fn add_to_ref_graph(&self, _: &mut RefGraph) {}
            #[inline]
            fn sweep(&self, _: bool, _: &mut RefGraph) {}
            #[inline]
            unsafe fn destroy_gcs(&mut self, _: &RefGraph) {}
        }
    };
}

collectable_trivial_impl!(());

collectable_trivial_impl!(u8);
collectable_trivial_impl!(u16);
collectable_trivial_impl!(u32);
collectable_trivial_impl!(u64);
collectable_trivial_impl!(u128);
collectable_trivial_impl!(usize);

collectable_trivial_impl!(i8);
collectable_trivial_impl!(i16);
collectable_trivial_impl!(i32);
collectable_trivial_impl!(i64);
collectable_trivial_impl!(i128);
collectable_trivial_impl!(isize);

collectable_trivial_impl!(f32);
collectable_trivial_impl!(f64);

collectable_trivial_impl!(AtomicUsize);
