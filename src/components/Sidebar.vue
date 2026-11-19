<script setup lang="ts">
import { ref, watch } from "vue";
import { RouterLink } from "vue-router";
import {
  LayoutDashboard,
  PanelLeftClose,
  PanelLeftOpen,
  Send,
  Settings2,
  ShieldCheck,
  Zap,
} from "lucide-vue-next";
import logoCarmine from "../assets/logo-carmine.png";
import logoSatellite from "../assets/logo-satellite.png";
import { appVersion } from "../store";

const COLLAPSED_KEY = "splitline.sidebar-collapsed";
const collapsed = ref(localStorage.getItem(COLLAPSED_KEY) === "1");

watch(collapsed, (value) => {
  localStorage.setItem(COLLAPSED_KEY, value ? "1" : "0");
});

const items = [
  { to: "/", id: "dashboard", label: "Главная", icon: LayoutDashboard },
  { to: "/zapret", id: "zapret", label: "Zapret", icon: ShieldCheck },
  { to: "/telegram", id: "telegram", label: "Telegram", icon: Send },
  { to: "/vpn", id: "vpn", label: "VPN", icon: Zap },
  { to: "/settings", id: "settings", label: "Настройки", icon: Settings2 },
];
</script>

<template>
  <aside class="sidebar" :class="{ collapsed }" aria-label="Основная навигация">
    <div class="sidebar-brand">
      <img class="brand-icon brand-icon-satellite" :src="logoSatellite" alt="" aria-hidden="true" />
      <img class="brand-icon brand-icon-carmine" :src="logoCarmine" alt="" aria-hidden="true" />
      <div v-if="!collapsed" class="brand-text">
        <strong>Splitline</strong>
      </div>
    </div>

    <nav class="sidebar-nav">
      <RouterLink
        v-for="item in items"
        :key="item.to"
        :to="item.to"
        class="nav-item"
        :data-icon="item.id"
        active-class=""
        exact-active-class="active"
        :title="collapsed ? item.label : undefined"
      >
        <component :is="item.icon" :size="18" :stroke-width="1.9" />
        <span class="nav-label" v-if="!collapsed">{{ item.label }}</span>
        <span v-else class="sr-only">{{ item.label }}</span>
      </RouterLink>
    </nav>

    <div class="sidebar-bottom">
      <div v-if="!collapsed" class="sidebar-version">
        {{ appVersion ? "версия " + appVersion : "версия —" }}
      </div>
      <button
        class="collapse-btn"
        type="button"
        :aria-label="collapsed ? 'Развернуть меню' : 'Свернуть меню'"
        @click="collapsed = !collapsed"
      >
        <PanelLeftOpen v-if="collapsed" :size="17" />
        <PanelLeftClose v-else :size="17" />
        <span v-if="!collapsed">Свернуть</span>
      </button>
    </div>
  </aside>
</template>
