<script setup lang="ts">
import { downloads } from "../store";
import type { DownloadProgress } from "../types";

function percent(item: DownloadProgress): number | null {
  if (item.total <= 0) return null;
  return Math.min(100, Math.round((item.received / item.total) * 100));
}

function sizeText(bytes: number): string {
  if (bytes <= 0) return "";
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} МБ`;
  return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
}
</script>

<template>
  <div class="download-overlay" aria-live="polite">
    <div
      v-for="item in downloads"
      :key="item.id"
      class="download-card"
      :class="{ error: item.error }"
    >
      <div class="download-head">
        <span class="download-label">{{ item.label }}</span>
        <span v-if="!item.error" class="download-value">
          {{ percent(item) === null ? sizeText(item.received) : `${percent(item)} %` }}
        </span>
      </div>
      <div v-if="!item.error" class="download-track">
        <div
          class="download-fill"
          :class="{ indeterminate: percent(item) === null }"
          :style="percent(item) === null ? undefined : { width: `${percent(item)}%` }"
        ></div>
      </div>
      <div v-else class="download-error">{{ item.error }}</div>
      <div v-if="!item.error && item.total > 0" class="download-sub">
        {{ sizeText(item.received) }} из {{ sizeText(item.total) }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.download-overlay {
  position: fixed;
  right: 18px;
  bottom: 18px;
  z-index: 70;
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 320px;
  max-width: calc(100vw - 36px);
  pointer-events: none;
}

.download-card {
  pointer-events: auto;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: color-mix(in oklab, var(--surface) 92%, transparent);
  box-shadow:
    0 12px 30px rgba(0, 0, 0, 0.35),
    inset 0 1px 0 rgba(255, 255, 255, 0.05);
  backdrop-filter: blur(14px);
}

.download-card.error {
  border-color: color-mix(in oklab, var(--danger) 60%, transparent);
}

.download-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
}

.download-label {
  overflow: hidden;
  color: var(--text);
  font-size: 11.5px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.download-value {
  flex: 0 0 auto;
  color: var(--text-3);
  font-family: var(--mono);
  font-size: 10.5px;
}

.download-track {
  height: 5px;
  margin-top: 7px;
  overflow: hidden;
  border-radius: 3px;
  background: color-mix(in oklab, var(--text-3) 24%, transparent);
}

.download-fill {
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(
    90deg,
    color-mix(in oklab, var(--accent) 55%, transparent),
    var(--accent)
  );
  transition: width 180ms ease;
}

.download-fill.indeterminate {
  width: 35%;
  animation: download-slide 1.1s ease-in-out infinite;
}

@keyframes download-slide {
  0% {
    transform: translateX(-100%);
  }

  100% {
    transform: translateX(320%);
  }
}

.download-sub {
  margin-top: 5px;
  color: var(--text-3);
  font-family: var(--mono);
  font-size: 10px;
}

.download-error {
  margin-top: 6px;
  color: var(--danger);
  font-size: 10.5px;
  line-height: 1.35;
}
</style>
