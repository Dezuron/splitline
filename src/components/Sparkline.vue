<script setup lang="ts">
import { computed } from "vue";

/** One line of the chart: `key` picks the colour class (`.down` / `.up`). */
export interface SparklineSeries {
  key: string;
  values: number[];
}

const props = withDefaults(defineProps<{ series: SparklineSeries[]; max?: number }>(), {
  max: 0,
});

const lines = computed(() => {
  const all = props.series.flatMap((item) => item.values);
  const max = props.max || Math.max(...all, 1);
  return props.series.map((item) => {
    const values = item.values.length ? item.values : [0];
    const step = values.length > 1 ? 100 / (values.length - 1) : 100;
    const points = values
      .map((value, index) => `${(index * step).toFixed(2)},${(28 - (value / max) * 26).toFixed(2)}`)
      .join(" ");
    return { key: item.key, points };
  });
});
</script>

<template>
  <svg class="sparkline" viewBox="0 0 100 30" preserveAspectRatio="none" aria-hidden="true">
    <polyline
      v-for="line in lines"
      :key="line.key"
      class="sparkline-series"
      :class="line.key"
      :points="line.points"
      fill="none"
      vector-effect="non-scaling-stroke"
    />
  </svg>
</template>

<style scoped>
.sparkline {
  display: block;
  width: 100%;
  height: 100%;
}

.sparkline-series {
  stroke: currentColor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.sparkline-series.down {
  color: var(--accent);
}

.sparkline-series.up {
  color: var(--text-3);
}
</style>
