<script setup lang="ts">
import { computed, onMounted } from "vue";
import Panel from "./Panel.vue";
import Button from "./ui/button/Button.vue";
import Toggle from "./Toggle.vue";
import MarkdownBody from "./MarkdownBody.vue";
import { formatBytes } from "../lib/labels";
import { tgwsVersionLabel } from "../lib/tgws";
import {
  applyAppUpdate,
  applyTgwsUpdate,
  applyUpdate,
  appUpdate,
  appUpdateProgress,
  appVersion,
  benchRunning,
  busy,
  checkAppUpdate,
  checkAllUpdates,
  checkTgwsUpdate,
  checkUpdate,
  checkingAppUpdate,
  checkingUpdate,
  chooseRuntimeFolder,
  config,
  installRuntime,
  openFolder,
  openReleasePage,
  runtimePath,
  runtimeReady,
  setAutoApply,
  setCheckAppUpdates,
  setGameFilter,
  setTheme,
  status,
  tgwsBusy,
  tgwsChecking,
  tgwsProgress,
  tgwsStatus,
  tgwsUpdate,
  toggleAutostart,
  toggleCheckUpdates,
  toggleFlashscore,
  updateInfo,
  updateProgress,
} from "../store";

const themes = [
  { value: "satellite" as const, label: "Спутник" },
  { value: "carmine" as const, label: "Кармин" },
];

const tgwsVersion = computed(() =>
  tgwsVersionLabel(
    config.value?.tgws.installed_version || tgwsStatus.value?.installed_version,
    tgwsStatus.value?.binary_present,
  ),
);

onMounted(() => {
  // Cached check: the Telegram page and this page share the same 12h cache, so
  // opening Settings never adds a GitHub round-trip on its own.
  void checkTgwsUpdate(true, false);
});
</script>

<template>
  <Panel title="Настройки">
    <section v-if="!runtimeReady" class="section">
      <h3>flowseal-zapret не установлен</h3>
      <p class="muted">
        Обход DPI появится после установки flowseal-zapret: около 10 МБ с GitHub, приложение
        скачает, распакует и предложит выбрать стратегию.
      </p>
      <Button :disabled="busy" @click="installRuntime">
        {{ busy ? "Установка…" : "Скачать и установить" }}
      </Button>
      <div v-if="updateProgress" class="muted">{{ updateProgress }}</div>
    </section>

    <section class="section">
      <div class="section-head">
        <h3>Обновления</h3>
        <Button
          variant="secondary"
          size="sm"
          :disabled="checkingAppUpdate || checkingUpdate || tgwsChecking || benchRunning"
          @click="checkAllUpdates"
        >
          {{ checkingAppUpdate || checkingUpdate || tgwsChecking ? "Проверка…" : "Проверить всё" }}
        </Button>
      </div>

      <div class="update-list">
        <div class="update-item">
          <div class="update-item-info">
            <strong>Приложение</strong>
            <span class="muted">
              {{ appVersion || "—" }}
              <template v-if="appUpdate?.available"> → {{ appUpdate.latest }}</template>
            </span>
          </div>
          <div class="update-item-actions">
            <Button
              v-if="appUpdate?.available"
              size="sm"
              :disabled="busy || benchRunning || !appUpdate.can_auto_update"
              @click="applyAppUpdate"
            >
              Обновить
            </Button>
            <template v-else>
              <Button
                variant="secondary"
                size="sm"
                :disabled="checkingAppUpdate || benchRunning"
                @click="checkAppUpdate(false)"
              >
                {{ checkingAppUpdate ? "Проверка…" : "Проверить" }}
              </Button>
            </template>
            <Button
              v-if="appUpdate?.available"
              variant="ghost"
              size="sm"
              @click="openReleasePage(appUpdate.url)"
            >
              Релиз
            </Button>
          </div>
          <details v-if="appUpdate?.available && appUpdate.notes" class="update-notes">
            <summary>Что нового</summary>
            <MarkdownBody :source="appUpdate.notes" />
          </details>
          <div v-if="appUpdateProgress" class="muted">{{ appUpdateProgress }}</div>
        </div>

        <div class="update-item">
          <div class="update-item-info">
            <strong>flowseal-zapret</strong>
            <span class="muted">
              {{ status?.installed_version || "не установлен" }}
              <template v-if="updateInfo?.update_available"> → {{ updateInfo.latest }}</template>
            </span>
          </div>
          <div class="update-item-actions">
            <Button
              v-if="updateInfo?.update_available"
              size="sm"
              :disabled="busy || benchRunning"
              @click="applyUpdate"
            >
              Обновить{{
                updateInfo.download_size ? ` (${formatBytes(updateInfo.download_size)})` : ""
              }}
            </Button>
            <Button
              v-else
              variant="secondary"
              size="sm"
              :disabled="checkingUpdate || benchRunning"
              @click="checkUpdate(false)"
            >
              {{ checkingUpdate ? "Проверка…" : "Проверить" }}
            </Button>
          </div>
          <details v-if="updateInfo?.update_available && updateInfo.changelog" class="update-notes">
            <summary>Что нового</summary>
            <MarkdownBody :source="updateInfo.changelog" />
          </details>
          <div v-if="updateProgress" class="muted">{{ updateProgress }}</div>
        </div>

        <div class="update-item">
          <div class="update-item-info">
            <strong>Telegram WS-прокси</strong>
            <span class="muted">
              {{ tgwsVersion }}
              <template v-if="tgwsUpdate?.has_update && tgwsUpdate.latest">
                → {{ tgwsUpdate.latest }}
              </template>
            </span>
          </div>
          <div class="update-item-actions">
            <Button
              v-if="tgwsUpdate?.has_update"
              size="sm"
              :disabled="tgwsBusy"
              @click="applyTgwsUpdate"
            >
              Обновить{{ tgwsUpdate.asset_size ? ` (${formatBytes(tgwsUpdate.asset_size)})` : "" }}
            </Button>
            <Button
              v-else
              variant="secondary"
              size="sm"
              :disabled="tgwsChecking"
              @click="checkTgwsUpdate(false, true)"
            >
              {{ tgwsChecking ? "Проверка…" : "Проверить" }}
            </Button>
          </div>
          <div v-if="tgwsProgress" class="muted">{{ tgwsProgress }}</div>
        </div>
      </div>
    </section>

    <section class="section">
      <h3>Настройки</h3>

      <div class="theme-field">
        <span class="field-label">Тема оформления</span>
        <div class="theme-picker">
          <button
            v-for="theme in themes"
            :key="theme.value"
            class="theme-option"
            :class="{ active: (config?.theme || 'dark') === theme.value }"
            type="button"
            @click="setTheme(theme.value)"
          >
            <span class="theme-preview" :data-theme-preview="theme.value" aria-hidden="true"></span>
            <span>{{ theme.label }}</span>
          </button>
        </div>
      </div>

      <Toggle
        label="Запуск при старте Windows"
        hint="Тихо при входе в систему"
        :checked="config?.autostart"
        :disabled="busy"
        @change="toggleAutostart"
      />
      <Toggle
        label="Проверять обновления приложения"
        hint="Проверка GitHub при запуске"
        :checked="config?.check_app_updates"
        @change="setCheckAppUpdates"
      />
      <Toggle
        label="Проверять обновления flowseal-zapret"
        hint="Проверка релизов Flowseal/zapret-discord-youtube при запуске"
        :checked="config?.check_updates"
        @change="toggleCheckUpdates"
      />
      <Toggle
        label="Обход Flashscore"
        hint="Домены Flashscore вместе с выбранной стратегией"
        :checked="config?.flashscore_bypass"
        :disabled="busy"
        @change="toggleFlashscore"
      />
      <Toggle
        label="Применять лучшую после проверки"
        hint="Автоматически включать стратегию с баллом 90 и выше"
        :checked="config?.bench_auto_apply"
        @change="setAutoApply"
      />

      <label class="field">
        <span class="field-label">Игровой фильтр</span>
        <select
          class="select"
          :value="config?.game_filter ?? 'disabled'"
          @change="setGameFilter(($event.target as HTMLSelectElement).value)"
        >
          <option value="disabled">Отключён</option>
          <option value="all">TCP + UDP</option>
          <option value="tcp">Только TCP</option>
          <option value="udp">Только UDP</option>
        </select>
      </label>

      <div class="field">
        <span class="field-label">Папка flowseal-zapret</span>
        <span class="field-value path-value" :title="runtimePath">{{ runtimePath }}</span>
        <Button
          variant="secondary"
          size="sm"
          title="Сменить папку; при запущенном обходе потребуется остановка"
          @click="chooseRuntimeFolder"
        >
          Изменить
        </Button>
      </div>

      <Button
        variant="secondary"
        size="sm"
        class="self-start"
        title="Открыть папку flowseal-zapret: стратегии .bat, списки и bin"
        @click="openFolder"
      >
        Открыть папку flowseal-zapret
      </Button>
    </section>
  </Panel>
</template>
