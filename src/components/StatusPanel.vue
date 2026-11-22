<script setup lang="ts">
import { BadgeCheck, ShieldCheck } from "lucide-vue-next";
import Button from "./ui/button/Button.vue";
import Card from "./ui/card/Card.vue";
import CardContent from "./ui/card/CardContent.vue";
import CardDescription from "./ui/card/CardDescription.vue";
import CardHeader from "./ui/card/CardHeader.vue";
import CardTitle from "./ui/card/CardTitle.vue";
import Switch from "./ui/switch/Switch.vue";
import {
  applyStrategy,
  benchRunning,
  busy,
  currentName,
  installRuntime,
  running,
  runtimeReady,
  selected,
  status,
  toggleStart,
  updateProgress,
  zapretStatusText,
  zapretTone,
} from "../store";
</script>

<template>
  <Card class="status-panel">
    <CardHeader class="status-hero-head">
      <div class="status-hero-icon"><ShieldCheck :size="21" /></div>
      <div class="status-hero-text">
        <span class="eyebrow">Zapret</span>
        <div class="status-hero-title">
          <CardTitle>Обход DPI</CardTitle>
          <span class="free-badge">
            <BadgeCheck :size="13" aria-hidden="true" />
            Бесплатно
          </span>
        </div>
        <CardDescription>{{ zapretStatusText }}</CardDescription>
      </div>
      <div class="status-hero-controls">
        <span class="status-dot" :class="zapretTone" aria-hidden="true"></span>
        <Switch
          :model-value="running"
          :disabled="busy || benchRunning || !runtimeReady"
          aria-label="Включить или выключить обход Zapret"
          @update:model-value="toggleStart"
        />
      </div>
    </CardHeader>

    <CardContent class="status-content">
      <div class="status-meta">
        <div>
          <span>Версия</span>
          <strong>{{ status?.installed_version || "не установлена" }}</strong>
        </div>
        <div>
          <span>Активная стратегия</span>
          <strong :title="currentName">{{ currentName || "—" }}</strong>
        </div>
      </div>

      <p v-if="running" class="muted">
        Служба запущена — это не проверка стратегии. Пробивает ли она блокировки, покажет «Текущая»
        в блоке «Стратегии».
      </p>

      <div v-if="selected && selected !== currentName" class="field-block selected-block">
        <span class="field-label">Выбрано</span>
        <div class="selected-row">
          <span class="field-value" :title="selected">{{ selected }}</span>
          <Button size="sm" :disabled="busy || benchRunning" @click="applyStrategy()">
            Применить
          </Button>
        </div>
      </div>

      <div v-if="!runtimeReady" class="onboarding">
        <p class="onboarding-title">Нужен flowseal-zapret</p>
        <p class="muted">
          Около 10 МБ с GitHub: скачается, распакуется и сразу будет готов к запуску.
        </p>
        <Button :disabled="busy" @click="installRuntime">
          {{ busy ? "Установка…" : "Скачать и установить" }}
        </Button>
        <div v-if="updateProgress" class="muted">{{ updateProgress }}</div>
      </div>

      <p v-if="benchRunning" class="hint">
        Идёт проверка стратегий — обход временно управляется автоматически.
      </p>
    </CardContent>
  </Card>
</template>

<style scoped>
.status-panel {
  border-color: var(--glass-border);
  background: linear-gradient(160deg, var(--glass-bg), var(--glass-bg-2));
  box-shadow:
    0 18px 50px var(--glass-shadow),
    inset 0 1px 0 var(--glass-highlight);
  backdrop-filter: blur(18px) saturate(135%);
}

.status-hero-head {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 12px;
}

.status-hero-icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  border-radius: 12px;
  border: 1px solid var(--glass-border);
  background: var(--accent-soft);
  color: var(--accent-text);
}

.status-hero-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.status-hero-title {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.status-hero-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-left: auto;
}

.status-content {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.status-meta {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: 8px;
}

.status-meta div {
  min-width: 0;
  padding: 9px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--muted);
}

.status-meta span {
  display: block;
  color: var(--text-3);
  font-size: 11.5px;
  font-weight: 500;
  letter-spacing: 0;
  text-transform: none;
}

.status-meta strong {
  display: block;
  overflow: hidden;
  color: var(--text);
  font-family: var(--mono);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
