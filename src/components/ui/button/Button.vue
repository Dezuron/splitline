<script setup lang="ts">
import { ref } from "vue";
import { cn } from "@/lib/utils";
import { buttonVariants, type ButtonVariants } from ".";

defineOptions({ inheritAttrs: false });

interface Props {
  variant?: ButtonVariants["variant"];
  size?: ButtonVariants["size"];
  class?: string;
  disabled?: boolean;
  type?: "button" | "submit" | "reset";
}

const props = withDefaults(defineProps<Props>(), {
  variant: "default",
  size: "default",
  type: "button",
  disabled: false,
});

const el = ref<HTMLButtonElement | null>(null);

/** Dialogs move focus here after opening. */
defineExpose({ focus: () => el.value?.focus() });
</script>

<template>
  <button
    ref="el"
    v-bind="$attrs"
    :type="props.type"
    :disabled="props.disabled"
    :data-variant="props.variant"
    :data-size="props.size"
    :class="cn(buttonVariants({ variant: props.variant, size: props.size }), props.class)"
  >
    <slot />
  </button>
</template>
