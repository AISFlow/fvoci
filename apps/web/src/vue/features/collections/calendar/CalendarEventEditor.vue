<script setup lang="ts">
// Adapted from official EventForm/EventPopover, with explicit guarded submit.
import { computed, ref, useId } from "vue";
import UButton from "@nuxt/ui/components/Button.vue";
import { t } from "@fvoci/i18n";
import { dateMovable } from "@/features/collections/calendar-model";
import { itemPath } from "@/lib/href";
import type { CollectionField } from "@/lib/queries/collections";
import { editorWrite, type CalendarEvent, type CalendarWrite } from "./calendar-adapter";
const props = defineProps<{ event: CalendarEvent; dateBy: string; fields: readonly CollectionField[]; zone: string; slug: string; pending: boolean; online: boolean; save: (row: CalendarEvent, write: CalendarWrite) => Promise<void> }>();
const emit = defineEmits<{ close: []; reload: [] }>();
const id = useId();
const raw = ref(props.event.local);
const timed = ref(props.event.timed);
const error = ref(false);
const invalid = ref(false);
const field = computed(() => props.fields.find(f => f.id === props.dateBy));
const isTimed = computed(() => props.dateBy === "due" ? timed.value : field.value?.type === "datetime");
const readOnly = computed(() => !dateMovable(props.dateBy, props.event, props.fields));
function toggleTimed(value: boolean) {
  timed.value = value;
  raw.value = value ? (raw.value.includes('T') ? raw.value : `${raw.value}T`) : raw.value.slice(0, 10);
}
async function submit(event: Event) {
  event.preventDefault();
  if (readOnly.value || props.pending || !props.online) return;
  const write = editorWrite(props.dateBy, props.event, raw.value, Boolean(isTimed.value), props.fields, props.zone);
  invalid.value = !write;
  if (!write) return;
  error.value = false;
  try { await props.save(props.event, write); emit("close"); } catch { error.value = true; }
}
</script>
<template>
  <form class="flex w-72 max-w-[calc(100vw-2rem)] flex-col gap-3 p-3" aria-label="Calendar event editor" @submit="submit" @keydown.esc.prevent="emit('close')" @keydown.enter="($event.isComposing || $event.keyCode === 229) && $event.preventDefault()">
    <a :href="itemPath(slug, event.displayId)" class="font-semibold hover:underline">{{ event.title }}</a>
    <label v-if="dateBy === 'due'" class="flex gap-2 text-sm"><input type="checkbox" :checked="timed" :disabled="readOnly || pending" @change="toggleTimed(($event.target as HTMLInputElement).checked)" />{{ t("task.activity.field.dueTime") }}</label>
    <label :for="id" class="text-sm">{{ t('collection.date') }} · {{ zone }}</label>
    <input :id="id" class="collection-select w-full" :type="isTimed ? 'datetime-local' : 'date'" v-model="raw" :disabled="readOnly || pending" />
    <p v-if="invalid || error" role="alert" class="text-sm text-error">{{ t('collection.saveError') }}</p>
    <UButton v-if="error" color="neutral" variant="outline" @click="emit('reload')">{{ t('collection.reloadView') }}</UButton>
    <div class="flex gap-2"><UButton type="submit" :disabled="readOnly || pending || !online">{{ t('collection.saveView') }}</UButton><UButton color="neutral" variant="ghost" @click="emit('close')">{{ t('common.cancel') }}</UButton></div>
  </form>
</template>
