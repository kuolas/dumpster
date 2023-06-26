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

use std::{
    alloc::{dealloc, Layout},
    cell::{Cell, RefCell},
    collections::{hash_map::Entry, HashMap, HashSet},
    num::NonZeroUsize,
    ops::Deref,
    ptr::{drop_in_place, NonNull, addr_of_mut},
};

use crate::{unsync::Gc, Collectable, Destroyer, OpaquePtr, Visitor};

use super::GcBox;

thread_local! {
    /// The global collection of allocation information for this thread.
    pub static DUMPSTER: Dumpster = Dumpster {
        to_collect: RefCell::new(HashMap::new()),
        n_ref_drops: Cell::new(0),
        n_refs_living: Cell::new(0),
    };
}

/// A dumpster is a collection of all the garbage that may or may not need to be cleaned up.
/// It also contains information relevant to when a sweep should be triggered.
pub struct Dumpster {
    /// A map from allocation IDs for allocations which may need to be collected to pointers to
    /// their allocations.
    to_collect: RefCell<HashMap<AllocationId, Cleanup>>,
    /// The number of times a reference has been dropped since the last collection was triggered.
    pub n_ref_drops: Cell<usize>,
    /// The number of references that currently exist in the entire heap and stack.
    pub n_refs_living: Cell<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// A unique identifier for an allocated garbage-collected block.
///
/// It contains a pointer to the reference count of the allocation.
pub struct AllocationId(pub NonNull<Cell<usize>>);

#[derive(Debug)]
/// The necessary information required to collect some garbage-collected data.
/// This data is stored in a map from allocation IDs to the necessary cleanup operation.
struct Cleanup {
    build_graph_fn: unsafe fn(OpaquePtr, &mut BuildRefGraph),
    sweep_fn: unsafe fn(OpaquePtr, &mut Sweep),
    destroy_gcs_fn: unsafe fn(OpaquePtr, &mut DestroyGcs),
    ptr: OpaquePtr,
}

impl Cleanup {
    fn new<T: Collectable + ?Sized>(box_ref: &GcBox<T>) -> Cleanup {
        dbg!(Cleanup {
            build_graph_fn: apply_visitor::<T, BuildRefGraph>,
            sweep_fn: apply_visitor::<T, Sweep>,
            destroy_gcs_fn: destroy_gcs::<T>,
            ptr: OpaquePtr::new(NonNull::from(box_ref)),
        })
    }
}

#[inline(never)]
unsafe fn apply_visitor<T: Collectable + ?Sized, V: Visitor>(ptr: OpaquePtr, visitor: &mut V) {
    println!("apply visitor!");
    let specified: NonNull<GcBox<T>> = ptr.specify();
    println!("done specifying - accept!");
    specified.as_ref().value.accept(visitor);
}

unsafe fn destroy_gcs<T: Collectable + ?Sized>(ptr: OpaquePtr, destroyer: &mut DestroyGcs) {
    let mut specific_ptr = ptr.specify::<GcBox<T>>();
    specific_ptr.as_mut().ref_count.set(0);
    specific_ptr
        .as_mut()
        .value
        .destroy_gcs(destroyer);

    destroyer.collection_queue.push((specific_ptr.as_ptr().cast(), Layout::for_value(&specific_ptr.as_ref().value)));
    drop_in_place(addr_of_mut!(specific_ptr.as_mut().value));
}

impl Dumpster {
    /// Collect all unreachable allocations that this dumpster is responsible for.
    pub fn collect_all(&self) {
        self.n_ref_drops.set(0);

        unsafe {
            let mut ref_graph_build = BuildRefGraph {
                visited: HashSet::new(),
                ref_state: HashMap::new(),
            };

            println!("building ref graph");
            for (k, v) in self.to_collect.borrow().iter() {
                if !ref_graph_build.visited.contains(k) {
                    ref_graph_build.visited.insert(*k);
                    println!("call build graph fn {:?}", v.build_graph_fn);
                    (v.build_graph_fn)(v.ptr, &mut ref_graph_build);
                }
            }
            println!("ref counts: {:?}", ref_graph_build.ref_state);

            let mut sweep = Sweep {
                visited: HashSet::new(),
            };
            for (id, v) in self.to_collect.borrow().iter() {
                let is_root = match ref_graph_build.ref_state.get(id) {
                    None => true,
                    Some(&n) => id.0.as_ref().get() != n.into(),
                };
                if is_root && !sweep.visited.insert(*id) {
                    (v.sweep_fn)(v.ptr, &mut sweep);
                }
            }

            println!("reachable: {:?}", sweep.visited);

            let mut destroy = DestroyGcs {
                visited: HashSet::new(),
                collection_queue: Vec::new(),
                reachable: sweep.visited,
            };
            // any allocation not found in the sweep must be freed
            for (id, cleanup) in self.to_collect.borrow_mut().drain() {
                if !destroy.reachable.contains(&id) && destroy.visited.insert(id) {
                    (cleanup.destroy_gcs_fn)(cleanup.ptr, &mut destroy);
                }
            }

            for (ptr, layout) in destroy.collection_queue {
                dealloc(ptr, layout);
            }

        }
    }

    /// Mark an allocation as "dirty," implying that it may need to be swept through later to find
    /// out if it has any references pointing to it.
    pub(super) fn mark_dirty<T: Collectable + ?Sized>(&self, box_ref: &GcBox<T>) {
        println!("mark {:?} as dirty", std::ptr::addr_of!(*box_ref));
        self.to_collect
            .borrow_mut()
            .entry(box_ref.id())
            .or_insert_with(|| Cleanup::new(box_ref));
    }

    /// Mark an allocation as "cleaned," implying that the allocation is about to be destroyed and
    /// therefore should not be cleaned up later.
    pub(super) fn mark_cleaned<T: Collectable + ?Sized>(&self, box_ref: &GcBox<T>) {
        println!("mark {:?} as cleaned", NonNull::from(box_ref));
        self.to_collect.borrow_mut().remove(&box_ref.id());
    }
}

impl Drop for Dumpster {
    fn drop(&mut self) {
        // cleanup any leftover allocations
        self.collect_all();
    }
}

struct BuildRefGraph {
    visited: HashSet<AllocationId>,
    ref_state: HashMap<AllocationId, NonZeroUsize>,
}

impl Visitor for BuildRefGraph {
    fn visit_sync<T>(&mut self, _: &crate::sync::Gc<T>)
    where
        T: Collectable + Sync + ?Sized,
    {
        // because `Gc` is `!Sync`, we know we won't find a `Gc` this way and can return
        // immediately.
    }

    fn visit_unsync<T>(&mut self, gc: &Gc<T>)
    where
        T: Collectable + ?Sized,
    {
        unsafe {
            let next_id = gc.ptr.unwrap().as_ref().id();
            match self.ref_state.entry(next_id) {
                Entry::Occupied(ref mut o) => {
                    *o.get_mut() = o.get().saturating_add(1);
                }
                Entry::Vacant(v) => {
                    v.insert(NonZeroUsize::MIN);
                }
            }
            if self.visited.insert(next_id) {
                gc.deref().accept(self);
            }
        }
    }
}

struct Sweep {
    visited: HashSet<AllocationId>,
}

impl Visitor for Sweep {
    fn visit_sync<T>(&mut self, _: &crate::sync::Gc<T>)
    where
        T: Collectable + Sync + ?Sized,
    {
        // because `Gc` is `!Sync`, we know we won't find a `Gc` this way and can return
        // immediately.
    }

    fn visit_unsync<T>(&mut self, gc: &Gc<T>)
    where
        T: Collectable + ?Sized,
    {
        unsafe {
            if self.visited.insert(gc.ptr.unwrap().as_ref().id()) {
                gc.deref().accept(self);
            }
        }
    }
}

struct DestroyGcs {
    visited: HashSet<AllocationId>,
    collection_queue: Vec<(*mut u8, Layout)>,
    reachable: HashSet<AllocationId>,
}

impl Destroyer for DestroyGcs {
    fn visit_sync<T>(&mut self, _: &mut crate::sync::Gc<T>)
    where
        T: Collectable + Sync + ?Sized,
    {
        // because `Gc` is `!Sync`, we know we won't find a `Gc` this way and can return
        // immediately.
    }

    fn visit_unsync<T>(&mut self, gc: &mut Gc<T>)
    where
        T: Collectable + ?Sized,
    {
        unsafe {
            if let Some(mut p) = gc.ptr {
                let id = p.as_ref().id();
                gc.ptr = None;
                if !self.reachable.contains(&id) && self.visited.insert(id) {
                    p.as_mut().ref_count.set(0);
                    p.as_mut().value.destroy_gcs(self);
                    self.collection_queue.push((id.0.as_ptr().cast(), Layout::for_value(p.as_ref())));
                    drop_in_place(addr_of_mut!(p.as_mut().value));
                }
                
            }
        }
    }
}
