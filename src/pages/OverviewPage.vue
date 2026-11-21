<script setup lang="ts">
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { ArrowUpRight, RefreshCw, Send, ShieldCheck, Zap } from "lucide-vue-next";
import Button from "../components/ui/button/Button.vue";
import Card from "../components/ui/card/Card.vue";
import CardContent from "../components/ui/card/CardContent.vue";
import CardDescription from "../components/ui/card/CardDescription.vue";
import CardHeader from "../components/ui/card/CardHeader.vue";
import CardTitle from "../components/ui/card/CardTitle.vue";
import Switch from "../components/ui/switch/Switch.vue";
import Sparkline from "../components/Sparkline.vue";
import { formatBytes, versionLabel } from "../lib/labels";
import {
  activeServiceCount,
  appUpdate,
  appVersion,
  benchRunning,
  busy,
  checkAllUpdates,
  checkingAppUpdate,
  checkingUpdate,
  config,
  running,
  runtimeReady,
  status,
  statusText,
  statusTone,
  tgwsActiveConnections,
  tgwsBusy,
  tgwsChecking,
  tgwsLastActivityLabel,
  tgwsRunning,
  tgwsStatus,
  tgwsStatusText,
  tgwsUpdate,
  toggleStart,
  toggleTgws,
  toggleXray,
  trafficDown,
  trafficSamples,
  trafficTotalDown,
  trafficTotalUp,
  trafficUp,
  tunnelErrors,
  tunnelOk,
  tunnelPing,
  tunnelSlo,
  updateInfo,
  vpnStatusText,
  xrayBusy,
  xrayStatus,
} from "../store";
import { tgwsEndpoint } from "../lib/tgws";

const router = useRouter();

const zapretState = computed(() => {
  if (!runtimeReady.value) return "Не установлен";
  return running.value ? "Работает" : "Выключен";
});

const appHasUpdate = computed(() => !!appUpdate.value?.available);
const zapretHasUpdate = computed(() => !!updateInfo.value?.update_available);
const tgwsHasUpdate = computed(() => !!tgwsUpdate.value?.has_update);

const updatesAvailable = computed(
  () => [appHasUpdate.value, zapretHasUpdate.value, tgwsHasUpdate.value].filter(Boolean).length,
);
const updatesLabel = computed(() => {
  const count = updatesAvailable.value;
  if (!count) return "Обновлений нет";
  return count === 1 ? "Доступно 1 обновление" : `Доступно обновлений: ${count}`;
});

/** Any of the three checks running: the versions-card button spins and ignores clicks. */
const checkingAllUpdates = computed(
  () => checkingAppUpdate.value || checkingUpdate.value || tgwsChecking.value,
);

/**
 * One click checks the app, flowseal-zapret and the Telegram proxy; the store
 * already reports the result as a toast, including "Обновлений нет."
 */
async function refreshUpdates() {
  if (checkingAllUpdates.value) return;
  await checkAllUpdates();
}

/** The Telegram dot respects a dismissed release; the button keeps its own count. */
const telegramUpdatePending = computed(
  () => !!tgwsUpdate.value?.has_update && !tgwsUpdate.value.dismissed,
);

interface HeroVersion {
  key: string;
  label: string;
  version: string;
  update: boolean;
}

/** What is installed where: one row of versions for the bottom card. */
const versions = computed<HeroVersion[]>(() => [
  {
    key: "splitline",
    label: "Splitline",
    version: versionLabel(appVersion.value),
    update: appHasUpdate.value,
  },
  {
    key: "zapret",
    label: "zapret",
    version: versionLabel(status.value?.installed_version),
    update: zapretHasUpdate.value,
  },
  {
    key: "xray",
    label: "Xray",
    version: versionLabel(xrayStatus.value?.installed_version),
    update: false,
  },
  {
    key: "telegram",
    label: "Telegram",
    version: versionLabel(tgwsStatus.value?.installed_version),
    update: telegramUpdatePending.value,
  },
]);

const vpnRunning = computed(() => xrayStatus.value?.state === "running");
const heroHint = computed(() =>
  activeServiceCount.value
    ? `Работает сервисов: ${activeServiceCount.value} из 3`
    : "Все сервисы выключены",
);

const telegramEndpoint = computed(() =>
  tgwsEndpoint(config.value?.tgws.host, config.value?.tgws.port),
);
const vpnProfile = computed(() => xrayStatus.value?.profile_name || "не выбрана");
const vpnEndpoint = computed(() => `127.0.0.1:${xrayStatus.value?.socks_port ?? 17808}`);

/** The chart shows the last minute: 30 samples at the 2 s poll interval. */
const CHART_SAMPLES = 30;
const SAMPLE_SECONDS = 2;

const chartSamples = computed(() => trafficSamples.value.slice(-CHART_SAMPLES));
const downSeries = computed(() => chartSamples.value.map((sample) => sample.down));
const upSeries = computed(() => chartSamples.value.map((sample) => sample.up));
const chartSeries = computed(() => [
  { key: "down", values: downSeries.value },
  { key: "up", values: upSeries.value },
]);
const trafficMax = computed(() => Math.max(1, ...downSeries.value, ...upSeries.value));
const windowLabel = computed(() => `${chartSamples.value.length * SAMPLE_SECONDS} с`);
const sessionBytes = computed(() => trafficTotalDown.value + trafficTotalUp.value);

/** Hover state: one index feeds both the time tip and the vertical cursor. */
const hoverIndex = ref<number | null>(null);
const hoverSample = computed(() =>
  hoverIndex.value === null ? null : (chartSamples.value[hoverIndex.value] ?? null),
);
const hoverExactLeft = computed(() => {
  const count = chartSamples.value.length;
  if (hoverIndex.value === null || count < 2) return 50;
  return (hoverIndex.value / (count - 1)) * 100;
});
/** The tip stays inside the plot; the cursor sits exactly on the sample. */
const hoverTipLeft = computed(() => Math.min(86, Math.max(14, hoverExactLeft.value)));
const hoverAgo = computed(() => {
  const count = chartSamples.value.length;
  if (hoverIndex.value === null) return "";
  const seconds = (count - 1 - hoverIndex.value) * SAMPLE_SECONDS;
  return seconds === 0 ? "сейчас" : `${seconds} с назад`;
});

function onChartMove(event: MouseEvent) {
  const count = chartSamples.value.length;
  if (!count) return;
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  if (!rect.width) return;
  const ratio = (event.clientX - rect.left) / rect.width;
  hoverIndex.value = Math.min(count - 1, Math.max(0, Math.round(ratio * (count - 1))));
}

function clearChartHover() {
  hoverIndex.value = null;
}

function speedLabel(bytes: number): string {
  return bytes < 1 ? "0 Б/с" : `${formatBytes(bytes)}/с`;
}
</script>

<template>
  <div class="main-page">
    <Card class="home-hero">
      <div class="home-hero-content">
        <span class="status-dot" :class="statusTone" aria-hidden="true"></span>
        <div class="home-hero-text">
          <strong>{{ statusText }}</strong>
          <span class="muted">{{ heroHint }}</span>
        </div>
      </div>
    </Card>

    <Card class="traffic-card">
      <div class="traffic-head">
        <div class="traffic-title-block">
          <span class="eyebrow">VPN</span>
          <strong class="traffic-title">Трафик через туннель</strong>
        </div>
        <span class="muted">только то, что идёт через VPN</span>
      </div>

      <div class="traffic-body">
        <div v-if="vpnRunning" class="traffic-numbers">
          <div class="traffic-number">
            <span class="traffic-arrow down" aria-hidden="true">↓</span>
            <strong>{{ speedLabel(trafficDown) }}</strong>
            <span class="traffic-word">загрузка</span>
          </div>
          <div class="traffic-number">
            <span class="traffic-arrow up" aria-hidden="true">↑</span>
            <strong>{{ speedLabel(trafficUp) }}</strong>
            <span class="traffic-word">отдача</span>
          </div>
          <span class="muted">
            Соединений: {{ xrayStatus?.client_connections ?? 0 }} · Сессия:
            {{ formatBytes(sessionBytes) || "0 Б" }}
          </span>
          <span class="muted">
            Пинг: {{ tunnelOk ? tunnelPing + " мс" : "—" }} · Ошибки: {{ tunnelErrors }} · SLO:
            {{ tunnelSlo }}%
          </span>
        </div>

        <div v-if="chartSamples.length" class="traffic-chart">
          <div class="traffic-legend">
            <span class="legend-item down">
              <span class="legend-dot" aria-hidden="true"></span>Загрузка
            </span>
            <span class="legend-item up">
              <span class="legend-dot" aria-hidden="true"></span>Отдача
            </span>
          </div>
          <div class="traffic-plot" @mousemove="onChartMove" @mouseleave="clearChartHover">
            <Sparkline :series="chartSeries" :max="trafficMax" />
            <span
              v-if="hoverSample"
              class="traffic-cursor"
              :style="{ left: hoverExactLeft + '%' }"
              aria-hidden="true"
            ></span>
            <div v-if="hoverSample" class="traffic-tip" :style="{ left: hoverTipLeft + '%' }">
              <span>{{ hoverAgo }}</span>
              <span
                >Загрузка <strong>{{ speedLabel(hoverSample.down) }}</strong></span
              >
              <span
                >Отдача <strong>{{ speedLabel(hoverSample.up) }}</strong></span
              >
            </div>
          </div>
          <div class="traffic-axis">
            <span>{{ windowLabel }} назад</span>
            <span>сейчас</span>
          </div>
        </div>
        <p v-else class="muted traffic-empty">
          {{
            vpnRunning
              ? "Ждём первый замер — данные обновляются каждые 2 секунды."
              : "Трафик появится, когда пойдёт через туннель."
          }}
        </p>
      </div>
    </Card>

    <div class="power-grid">
      <Card class="power-card">
        <CardHeader class="power-head">
          <div class="power-icon zapret"><ShieldCheck :size="20" /></div>
          <div class="power-heading">
            <CardTitle>Zapret</CardTitle>
            <CardDescription>{{ zapretState }}</CardDescription>
          </div>
          <Switch
            :model-value="running"
            :disabled="busy || benchRunning || !runtimeReady"
            aria-label="Включить или выключить обход Zapret"
            @update:model-value="toggleStart"
          />
        </CardHeader>
        <CardContent class="power-content">
          <div class="power-meta">
            <span>Стратегия</span>
            <strong>{{ status?.current_strategy || "не выбрана" }}</strong>
          </div>
          <Button
            v-if="!runtimeReady"
            class="power-action"
            variant="secondary"
            size="sm"
            @click="router.push('/zapret')"
          >
            Установить Zapret
          </Button>
          <Button
            v-else
            class="power-action"
            variant="outline"
            size="sm"
            @click="router.push('/zapret')"
          >
            Стратегии и проверка
            <ArrowUpRight :size="13" />
          </Button>
        </CardContent>
      </Card>

      <Card class="power-card">
        <CardHeader class="power-head">
          <div class="power-icon telegram"><Send :size="19" /></div>
          <div class="power-heading">
            <CardTitle>Telegram</CardTitle>
            <CardDescription>{{ tgwsStatusText }}</CardDescription>
          </div>
          <Switch
            :model-value="tgwsRunning"
            :disabled="tgwsBusy"
            aria-label="Включить или выключить Telegram WS-прокси"
            @update:model-value="toggleTgws"
          />
        </CardHeader>
        <CardContent class="power-content">
          <div class="power-meta">
            <span>Адрес</span>
            <strong>{{ telegramEndpoint }}</strong>
          </div>
          <div class="power-meta">
            <span>Подключений</span>
            <strong>{{ tgwsActiveConnections ?? "—" }}</strong>
          </div>
          <div class="power-meta">
            <span>Последняя активность</span>
            <strong>{{ tgwsLastActivityLabel }}</strong>
          </div>
          <Button
            class="power-action"
            variant="outline"
            size="sm"
            @click="router.push('/telegram')"
          >
            Подключение и ключ
            <ArrowUpRight :size="13" />
          </Button>
        </CardContent>
      </Card>

      <Card class="power-card">
        <CardHeader class="power-head">
          <div class="power-icon telegram"><Zap :size="19" /></div>
          <div class="power-heading">
            <CardTitle>VPN</CardTitle>
            <CardDescription>{{ vpnStatusText }}</CardDescription>
          </div>
          <Switch
            :model-value="vpnRunning"
            :disabled="xrayBusy"
            aria-label="Включить или выключить VPN"
            @update:model-value="toggleXray"
          />
        </CardHeader>
        <CardContent class="power-content">
          <div class="power-meta">
            <span>Локация</span>
            <strong>{{ vpnProfile }}</strong>
          </div>
          <div class="power-meta">
            <span>SOCKS5</span>
            <strong>{{ vpnEndpoint }}</strong>
          </div>
          <Button class="power-action" variant="outline" size="sm" @click="router.push('/vpn')">
            Подписка и локации
            <ArrowUpRight :size="13" />
          </Button>
        </CardContent>
      </Card>
    </div>

    <Card class="versions-card">
      <div class="versions-head">
        <span class="eyebrow">Компоненты</span>
        <strong>Версии</strong>
      </div>
      <ul class="versions-list">
        <li v-for="item in versions" :key="item.key" class="home-version">
          <span v-if="item.update" class="home-version-dot" aria-hidden="true"></span>
          <span class="home-version-label">{{ item.label }}</span>
          <strong class="home-version-value">{{ item.version }}</strong>
          <span v-if="item.update" class="sr-only">— доступно обновление</span>
        </li>
      </ul>
      <div class="versions-actions">
        <Button
          v-if="updatesAvailable"
          variant="secondary"
          size="sm"
          @click="router.push('/settings')"
        >
          {{ updatesLabel }}
        </Button>
        <Button
          variant="outline"
          size="icon"
          title="Проверить обновления"
          aria-label="Проверить обновления"
          :disabled="checkingAllUpdates"
          @click="refreshUpdates"
        >
          <RefreshCw :size="15" :class="{ spinning: checkingAllUpdates }" />
        </Button>
      </div>
    </Card>
  </div>
</template>
