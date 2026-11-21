// Highway is vendored under vendor/highway; see its LICENSE and provenance.
// This translation unit is compiled separately for AVX2 and optionally AVX512.
#define HWY_HEADER_ONLY
#define HWY_COMPILE_ONLY_STATIC
#define VQSORT_ONLY_STATIC 1
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include "hwy/contrib/sort/vqsort-inl.h"

#ifndef SHOPPING_SORT_NAME
#define SHOPPING_SORT_NAME shopping_sort128_avx2
#endif

// Preconditions: exclusive initialized storage holding count 16-byte records,
// no padding, and each record's low/high uint64 lanes form an unsigned key.
// The caller retains no usable references during this synchronous call.
extern "C" int SHOPPING_SORT_NAME(void* storage, std::size_t count) noexcept {
  if (count < 2) return 1;
  if (storage == nullptr || (reinterpret_cast<std::uintptr_t>(storage) & 15) ||
      count > std::numeric_limits<std::size_t>::max() / 16) return 0;

  // C++20 memmove implicitly creates suitable implicit-lifetime objects in its
  // destination (WG21 P0593R6). Using its returned pointer establishes uint64
  // array objects here without type-punning Event or hwy::uint128_t pointers.
  // Identical source/destination is permitted and optimizes to no byte copy.
  auto* lanes = static_cast<std::uint64_t*>(
      std::memmove(storage, storage, count * 16));
  const hwy::HWY_NAMESPACE::detail::MakeTraits<hwy::uint128_t,
                                               hwy::SortAscending> traits;
  const hwy::HWY_NAMESPACE::SortTag<std::uint64_t> d;
  // This is the same lane-based kernel invoked by VQSortStatic<uint128_t>.
  // It moves complete two-lane keys; the high lane is the primary comparison.
  hwy::HWY_NAMESPACE::Sort(d, traits, lanes, count * 2);
  return 1;
}
