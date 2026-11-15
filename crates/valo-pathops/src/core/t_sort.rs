//! Port of Skia's src/base/SkTSort.h: introsort (quicksort, falling back to heapsort, finishing
//! with insertion sort).
//!
//! The sorts are unstable, and PathOps' results depend on the order they leave equal keys in,
//! so these must be used wherever Skia calls them, never std's sorts. Skia's pointer ranges
//! are slices here; its 1-based heap indices are kept.

use crate::core::float_bits::sk_next_log2;

fn sk_t_heap_sort_sift_up<T: Copy, C: Fn(&T, &T) -> bool>(
    array: &mut [T],
    mut root: usize,
    bottom: usize,
    less_than: &C,
) {
    let x = array[root - 1];
    let start = root;
    let mut j = root << 1;
    while j <= bottom {
        if j < bottom && less_than(&array[j - 1], &array[j]) {
            j += 1;
        }
        array[root - 1] = array[j - 1];
        root = j;
        j = root << 1;
    }
    j = root >> 1;
    while j >= start {
        if less_than(&array[j - 1], &x) {
            array[root - 1] = array[j - 1];
            root = j;
            j = root >> 1;
        } else {
            break;
        }
    }
    array[root - 1] = x;
}

fn sk_t_heap_sort_sift_down<T: Copy, C: Fn(&T, &T) -> bool>(
    array: &mut [T],
    mut root: usize,
    bottom: usize,
    less_than: &C,
) {
    let x = array[root - 1];
    let mut child = root << 1;
    while child <= bottom {
        if child < bottom && less_than(&array[child - 1], &array[child]) {
            child += 1;
        }
        if less_than(&x, &array[child - 1]) {
            array[root - 1] = array[child - 1];
            root = child;
            child = root << 1;
        } else {
            break;
        }
    }
    array[root - 1] = x;
}

/// `SkTHeapSort(array, count, lessThan)`.
pub(crate) fn sk_t_heap_sort<T: Copy, C: Fn(&T, &T) -> bool>(array: &mut [T], less_than: &C) {
    let count = array.len();
    let mut i = count >> 1;
    while i > 0 {
        sk_t_heap_sort_sift_down(array, i, count, less_than);
        i -= 1;
    }
    let mut i = count.saturating_sub(1);
    while i > 0 {
        array.swap(0, i);
        sk_t_heap_sort_sift_up(array, 1, i, less_than);
        i -= 1;
    }
}

/// `SkTInsertionSort(left, count, lessThan)`.
pub(crate) fn sk_t_insertion_sort<T: Copy, C: Fn(&T, &T) -> bool>(array: &mut [T], less_than: &C) {
    let count = array.len();
    if count == 0 {
        return;
    }
    let right = count - 1;
    let mut next = 1;
    while next <= right {
        if !less_than(&array[next], &array[next - 1]) {
            next += 1;
            continue;
        }
        let insert = array[next];
        let mut hole = next;
        loop {
            array[hole] = array[hole - 1];
            hole -= 1;
            if !(0 < hole && less_than(&insert, &array[hole - 1])) {
                break;
            }
        }
        array[hole] = insert;
        next += 1;
    }
}

/// `SkTQSort_Partition(left, count, pivot, lessThan)`: returns the new pivot's index.
fn sk_t_q_sort_partition<T: Copy, C: Fn(&T, &T) -> bool>(
    array: &mut [T],
    pivot: usize,
    less_than: &C,
) -> usize {
    let right = array.len() - 1;
    let pivot_value = array[pivot];
    array.swap(pivot, right);
    let mut new_pivot = 0;
    let mut left = 0;
    while left < right {
        if less_than(&array[left], &pivot_value) {
            array.swap(left, new_pivot);
            new_pivot += 1;
        }
        left += 1;
    }
    array.swap(new_pivot, right);
    new_pivot
}

/// `SkTIntroSort(depth, left, count, lessThan)`.
fn sk_t_intro_sort<T: Copy, C: Fn(&T, &T) -> bool>(
    mut depth: i32,
    mut array: &mut [T],
    less_than: &C,
) {
    loop {
        let count = array.len();
        if count <= 32 {
            sk_t_insertion_sort(array, less_than);
            return;
        }
        if depth == 0 {
            sk_t_heap_sort(array, less_than);
            return;
        }
        depth -= 1;

        let middle = (count - 1) >> 1;
        let pivot_count = sk_t_q_sort_partition(array, middle, less_than);

        let (lower, upper) = array.split_at_mut(pivot_count);
        sk_t_intro_sort(depth, lower, less_than);
        array = &mut upper[1..];
    }
}

/// `SkTQSort(begin, end, lessThan)`.
pub(crate) fn sk_t_q_sort_by<T: Copy, C: Fn(&T, &T) -> bool>(array: &mut [T], less_than: C) {
    let n = array.len() as i32;
    if n <= 1 {
        return;
    }
    // Limit Introsort recursion depth to no more than 2 * ceil(log2(n-1)).
    let depth = 2 * sk_next_log2((n - 1) as u32);
    sk_t_intro_sort(depth, array, &less_than);
}

/// `SkTQSort(begin, end)`: sorts with `a < b`.
pub(crate) fn sk_t_q_sort<T: Copy + PartialOrd>(array: &mut [T]) {
    sk_t_q_sort_by(array, |a: &T, b: &T| a < b);
}
