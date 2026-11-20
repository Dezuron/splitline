<script setup lang="ts">
import { computed } from "vue";
import { useRouter } from "vue-router";
import { ArrowUpRight, Send, ShieldCheck, Zap } from "lucide-vue-next";
import Button from "../components/ui/button/Button.vue";
import Card from "../components/ui/card/Card.vue";
import CardContent from "../components/ui/card/CardContent.vue";
import CardDescription from "../components/ui/card/CardDescription.vue";
import CardHeader from "../components/ui/card/CardHeader.vue";
import CardTitle from "../components/ui/card/CardTitle.vue";
import Switch from "../components/ui/switch/Switch.vue";
import Sparkline from "../components/Sparkline.vue";
import { formatBytes } from "../lib/labels";
import {
  appUpdate,
  benchRunning,
  busy,
  config,
  running,
  runtimeReady,
  status,
  statusText,
  statusTone,
  tgwsBusy,
  tgwsRunning,
  tgwsStateText,
  tgwsStatus,
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
  xrayBusy,
  xrayStatus,
} from "../store";
import { tgwsEndpoint } from "../lib/tgws";

const router = useRouter();

const zapretState = computed(() => {
  if (!runtimeReady.value) return "Не установлен";
  return running.value ? "Включён" : "Выключен";
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

const vpnRunning = computed(() => xrayStatus.value?.state === "running");
const activeServices = computed(
  () =>
    [runtimeReady.value && running.value, tgwsRunning.value, vpnRunning.value].filter(Boolean)
      .length,
);
const heroHint = computed(() =>
  activeServices.value ? `Активно сервисов: ${activeServices.value} из 3` : "Все сервисы выключены",
);

const telegramEndpoint = computed(() =>
  tgwsEndpoint(config.value?.tgws.host, config.value?.tgws.port),
);
const vpnHint = computed(() => {
  if (!xrayStatus.value) return "Загрузка…";
  if (!vpnRunning.value) return "VPN выключен";
  const connections = xrayStatus.value.client_connections ?? 0;
  return connections > 0 ? "Telegram через VPN" : "VPN подключён";
});
const vpnProfile = computed(() => xrayStatus.value?.profile_name || "не выбрана");
const vpnEndpoint = computed(() => `127.0.0.1:${xrayStatus.value?.socks_port ?? 17808}`);

const telegramHint = computed(() => {
  if (!tgwsRunning.value) return tgwsStateText.value;
  return (tgwsStatus.value?.client_connections_total ?? 0) > 0
    ? "Прокси включён"
    : "Прокси включён · Telegram не подключён";
});

const downSeries = computed(() => trafficSamples.value.map((sample) => sample.down));
const upSeries = computed(() => trafficSamples.value.map((sample) => sample.up));
const trafficMax = computed(() => Math.max(1, ...downSeries.value, ...upSeries.value));
const sessionBytes = computed(() => trafficTotalDown.value + trafficTotalUp.value);

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
        <Button
          v-if="updatesAvailable"
          variant="secondary"
          size="sm"
          @click="router.push('/settings')"
        >
          {{ updatesLabel }}
        </Button>
      </div>
    </Card>

    <Card v-if="vpnRunning" class="traffic-card">
      <div class="traffic-head">
        <div class="traffic-title-block">
          <span class="eyebrow">VPN</span>
          <strong class="traffic-title">Трафик через туннель</strong>
        </div>
        <span class="muted">только то, что идёт через VPN</span>
      </div>

      <div class="traffic-body">
        <div class="traffic-numbers">
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

        <div v-if="trafficSamples.length" class="traffic-chart">
          <div
            class="traffic-series down"
            title="Загрузка: сколько данных приходит из туннеля в секунду"
          >
            <span class="series-name">Загрузка</span>
            <Sparkline
              :values="downSeries"
              :max="trafficMax"
              class="traffic-down"
              style="height: 26px"
            />
          </div>
          <div class="traffic-series up" title="Отдача: сколько данных уходит в туннель в секунду">
            <span class="series-name">Отдача</span>
            <Sparkline
              :values="upSeries"
              :max="trafficMax"
              class="traffic-up"
              style="height: 26px"
            />
          </div>
        </div>
        <p v-else class="muted traffic-empty">
          Ждём первый замер — данные обновляются каждые 2 секунды.
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
            <CardDescription>{{ telegramHint }}</CardDescription>
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
            <CardDescription>{{ vpnHint }}</CardDescription>
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
  </div>
</template>
