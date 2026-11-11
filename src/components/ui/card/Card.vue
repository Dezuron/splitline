<script setup lang="ts">
import { cn } from "@/lib/utils";

const props = defineProps<{ class?: string }>();
</script>

<template>
  <div
    data-slot="card"
    :class="
      cn(
        'rounded-[var(--radius-lg)] border border-border bg-card text-card-foreground shadow-[var(--card-shadow)]',
        props.class,
      )
    "
  >
    <slot />
  </div>
</template>
