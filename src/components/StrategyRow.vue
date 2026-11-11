<script setup lang="ts">
import type { StrategyRowData } from "../store";
import { verdictLabel } from "../lib/labels";

const props = defineProps<{
  row: StrategyRowData;
  selected: boolean;
  best: boolean;
  disabled: boolean;
}>();

const emit = defineEmits<{
  select: [];
  apply: [];
  delete: [];
}>();

function onKeydown(event: KeyboardEvent) {
  // Keys pressed on the nested Apply/Delete buttons must not select the row.
  if (event.target !== event.currentTarget) return;
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    emit("select");
  }
}
</script>

<template>
  <div
    class="strategy-row"
    :class="{
      selected: props.selected,
      active: props.row.active,
      best: props.best,
      generated: props.row.generated,
    }"
    role="button"
    tabindex="0"
    :aria-pressed="props.selected"
    @click="emit('select')"
    @keydown="onKeydown"
  >
    <div class="row-main">
      <div class="row-title">
        <span class="row-name">{{ props.row.name }}</span>
        <span v-if="props.row.active" class="chip chip-live">активна</span>
        <span v-if="props.row.generated" class="chip chip-gen">эксперим.</span>
        <span v-if="props.best" class="star" title="Лучший результат" aria-label="Лучший результат">
          ★
        </span>
      </div>

      <div v-if="props.row.bench" class="row-metrics">
        <span class="metric">
          <b>{{ props.row.bench.discord_ok }}/{{ props.row.bench.discord_total }}</b> Discord
        </span>
        <span class="metric">
          <b>{{ props.row.bench.youtube_ok }}/{{ props.row.bench.youtube_total }}</b> YouTube
        </span>
        <span
          class="metric"
          :class="{ bad: props.row.bench.control_ok < props.row.bench.control_total }"
        >
          <b>{{ props.row.bench.control_ok }}/{{ props.row.bench.control_total }}</b> контроль
        </span>
        <span class="metric">
          <b>{{ props.row.bench.avg_ms || "—" }}</b> мс
        </span>
        <span class="verdict" :class="props.row.bench.verdict">
          {{ verdictLabel[props.row.bench.verdict] }}
        </span>
      </div>
      <div v-else-if="props.row.generated" class="row-preview muted">
        создана из «{{ props.row.parent }}» · нет данных
      </div>
      <div v-else class="row-preview">
        {{ props.row.args_preview || "нет данных" }}
      </div>
    </div>

    <div class="row-side">
      <span
        v-if="props.row.bench"
        class="score"
        :class="props.row.bench.verdict"
        :title="`${props.row.bench.score}/100 · ${verdictLabel[props.row.bench.verdict]}`"
      >
        {{ props.row.bench.score }}
      </span>
      <button
        class="icon-btn row-apply"
        :disabled="props.disabled || props.row.active"
        :aria-label="`Применить стратегию ${props.row.name}`"
        title="Применить"
        @click.stop="emit('apply')"
      >
        →
      </button>
      <button
        v-if="props.row.generated"
        class="icon-btn danger"
        :disabled="props.disabled"
        :aria-label="`Удалить стратегию ${props.row.name}`"
        title="Удалить"
        @click.stop="emit('delete')"
      >
        ×
      </button>
    </div>
  </div>
</template>
