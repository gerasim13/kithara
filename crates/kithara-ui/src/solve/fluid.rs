use super::flex::Item;
use crate::layout::Axis;

#[derive(Clone, Copy, Default)]
pub(super) struct Allocation {
    extent: f32,
    is_pinned: bool,
}

impl Allocation {
    pub(super) const fn extent(self) -> f32 {
        self.extent
    }
}

pub(super) fn allocate(
    items: &[Item],
    axis: Axis,
    remaining: f32,
    fill_sum: u16,
) -> Vec<Allocation> {
    let minimum_sum = items
        .iter()
        .filter(|item| fill_factor(item, axis) != 0)
        .map(minimum)
        .sum::<f32>();
    let mut allocations = vec![Allocation::default(); items.len()];

    if minimum_sum > remaining {
        let scale = if minimum_sum > 0.0 {
            remaining / minimum_sum
        } else {
            0.0
        };

        for (item, allocation) in items.iter().zip(&mut allocations) {
            if fill_factor(item, axis) != 0 {
                allocation.extent = minimum(item) * scale;
            }
        }
        return allocations;
    }

    let mut pool = remaining;
    let mut weight_sum = f32::from(fill_sum);

    loop {
        let next =
            items
                .iter()
                .zip(&allocations)
                .enumerate()
                .find_map(|(index, (item, allocation))| {
                    if allocation.is_pinned {
                        return None;
                    }
                    let weight = fill_factor(item, axis);
                    if weight == 0 {
                        return None;
                    }
                    let item_minimum = minimum(item);
                    let share = pool * f32::from(weight) / weight_sum;
                    (share < item_minimum).then_some((index, item_minimum, weight))
                });
        let Some((index, minimum, weight)) = next else {
            break;
        };

        allocations[index] = Allocation {
            extent: minimum,
            is_pinned: true,
        };
        pool -= minimum;
        weight_sum -= f32::from(weight);
    }

    for (item, allocation) in items.iter().zip(&mut allocations) {
        let weight = fill_factor(item, axis);
        if weight != 0 && !allocation.is_pinned {
            allocation.extent = pool * f32::from(weight) / weight_sum;
        }
    }

    allocations
}

fn fill_factor(item: &Item, axis: Axis) -> u16 {
    match axis {
        Axis::Horizontal => item.declared.width.fill_factor(),
        Axis::Vertical => item.declared.height.fill_factor(),
    }
}

fn minimum(item: &Item) -> f32 {
    item.main_minimum.unwrap_or(0.0).max(0.0)
}
