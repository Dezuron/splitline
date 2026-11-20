<script setup lang="ts">
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    values: number[];
    max?: number;
  }>(),
  { max: 0 },
);

const points = computed(() => {
  const values = props.values.length ? props.values : [0];
  const max = props.max || Math.max(...values, 1);
  const step = values.length > 1 ? 100 / (values.length - 1) : 100;
  return values
    .map((value, index) => `${(index * step).toFixed(2)},${(28 - (value / max) * 26).toFixed(2)}`)
    .join(" ");
});
</script>

<template>
  <svg class="sparkline" viewBox="0 0 100 30" preserveAspectRatio="none" aria-hidden="true">
    <polyline :points="points" fill="none" vector-effect="non-scaling-stroke" />
  </svg>
</template>

<style scoped>
.sparkline {
  display: block;
  width: 100%;
  height: 32px;
  color: var(--accent);
}

polyline {
  stroke: currentColor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}
</style>
