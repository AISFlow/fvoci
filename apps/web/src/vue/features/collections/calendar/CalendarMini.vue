<script setup lang="ts">
// Adapted from official CalendarMini/AppSidebar: date selection in a compact sidebar.
import { computed } from "vue";
import { monthGrid } from "@/lib/collection-values";
import { weekdayNames } from "@/features/collections/collection-view";
const props = defineProps<{
  month: string;
  selected: string;
  today: string;
  weekStartsOn: number;
}>();
const emit = defineEmits<{ select: [date: string] }>();
const weeks = computed(() => monthGrid(props.month, props.weekStartsOn));
</script>
<template>
  <table class="w-full text-center text-xs" aria-label="Mini calendar" data-testid="calendar-mini">
    <thead
      ><tr
        ><th v-for="name in weekdayNames(weekStartsOn)" :key="name" class="py-2 text-muted">{{
          name
        }}</th></tr
      ></thead
    >
    <tbody
      ><tr v-for="week in weeks" :key="week[0]!.date"
        ><td v-for="cell in week" :key="cell.date">
          <button
            type="button"
            class="size-7 rounded-full hover:bg-elevated focus-visible:outline-2"
            :class="
              cell.date === selected
                ? 'bg-primary text-inverted'
                : !cell.inMonth
                  ? 'text-dimmed'
                  : ''
            "
            :aria-label="cell.date"
            :aria-pressed="cell.date === selected"
            :aria-current="cell.date === today ? 'date' : undefined"
            @click="emit('select', cell.date)"
            >{{ Number(cell.date.slice(8)) }}</button
          >
        </td></tr
      ></tbody
    >
  </table>
</template>
