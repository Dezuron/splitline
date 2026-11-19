<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import Button from "./ui/button/Button.vue";
import {
  busy,
  chooseRuntimeFolder,
  dismissSetupWizard,
  installRuntime,
  runtimePath,
  updateProgress,
  useDefaultRuntimeFolder,
} from "../store";

const step = ref<1 | 2 | 3>(1);

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") dismissSetupWizard();
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <div class="setup-wizard" role="dialog" aria-modal="true" aria-label="Мастер настройки Splitline">
    <div class="setup-card">
      <div class="setup-steps" aria-hidden="true">
        <span :class="{ active: step >= 1 }">1</span>
        <span :class="{ active: step >= 2 }">2</span>
        <span :class="{ active: step >= 3 }">3</span>
      </div>

      <div v-show="step === 1">
        <h2>Добро пожаловать в Splitline</h2>
        <p class="setup-text">
          Приложение управляет обходом DPI для Discord и YouTube. Для работы нужен flowseal-zapret
          (zapret-discord-youtube от Flowseal): около 10 МБ, скачивается с GitHub и распаковывается
          локально.
        </p>
        <ul class="setup-list">
          <li>Windows 11 или Windows 10 22H2 (сборка 19045.6456 и новее)</li>
          <li>Права администратора при запуске — нужны для WinDivert</li>
          <li>
            WebView2 Runtime — в Windows 11 уже установлен, на Windows 10 приложение проверит его
            наличие
          </li>
        </ul>
        <div class="setup-actions">
          <Button variant="secondary" size="sm" @click="dismissSetupWizard">Настроить позже</Button>
          <Button @click="step = 2">Продолжить</Button>
        </div>
      </div>

      <div v-show="step === 2">
        <h2>Где хранить flowseal-zapret?</h2>
        <p class="setup-text">
          В выбранной папке будут стратегии <code>.bat</code>, списки и <code>bin/winws.exe</code>.
          Можно оставить путь по умолчанию или выбрать другую папку — например, рядом с портативным
          exe.
        </p>

        <div class="setup-path" :title="runtimePath">{{ runtimePath || "Путь по умолчанию" }}</div>

        <div class="setup-actions setup-actions-wrap">
          <Button variant="secondary" size="sm" @click="step = 1">Назад</Button>
          <Button variant="secondary" size="sm" @click="useDefaultRuntimeFolder">
            По умолчанию
          </Button>
          <Button variant="secondary" size="sm" @click="chooseRuntimeFolder">Изменить</Button>
          <Button @click="step = 3">Продолжить</Button>
        </div>
      </div>

      <div v-show="step === 3">
        <h2>Скачать flowseal-zapret</h2>
        <p class="setup-text">Файл будет скачан с GitHub, проверен по SHA-256 и распакован в:</p>
        <div class="setup-path" :title="runtimePath">{{ runtimePath || "Путь по умолчанию" }}</div>

        <div v-if="updateProgress" class="muted setup-progress">{{ updateProgress }}</div>

        <div class="setup-actions setup-actions-wrap">
          <Button variant="secondary" size="sm" :disabled="busy" @click="step = 2">Назад</Button>
          <Button :disabled="busy" @click="installRuntime">
            {{ busy ? "Установка…" : "Скачать и установить" }}
          </Button>
        </div>
      </div>
    </div>
  </div>
</template>
