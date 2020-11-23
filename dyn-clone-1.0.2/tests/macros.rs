use dyn_clone::{clone_trait_object, DynClone};

fn assert_clone<T: Clone>() {}

#[test]
fn test_plain() {
    trait Trait: DynClone {}

    clone_trait_object!(Trait);

    assert_clone::<Box<dyn Trait>>();
}

#[test]
fn test_type_parameter() {
    trait Trait<T>: DynClone {}

    clone_trait_object!(<T> Trait<T>);

    assert_clone::<Box<dyn Trait<u32>>>();
}

#[test]
fn test_generic_bound() {
    trait Trait<T: PartialEq<T>, U>: DynClone {}

    clone_trait_object!(<T: PartialEq<T>, U> Trait<T, U>);

    assert_clone::<Box<dyn Trait<u32, ()>>>();
}

#[test]
fn test_where_clause() {
    trait Trait<T>: DynClone where T: Clone {}

    clone_trait_object!(<T> Trait<T> where T: Clone);

    assert_clone::<Box<dyn Trait<u32>>>();
}
