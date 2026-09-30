<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import ConfirmAction from "../../components/ConfirmAction.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import { inputChecked, inputText } from "../../composables/useZodForm";
import type { components } from "@/generated/api";
import { problemMessage } from "@/lib/api";
import {
  BRANDING_ASSET_KINDS,
  BRANDING_ASSET_MIME,
  type BrandingAssetKind,
  EE_GATED,
  SETTING_ENUM_OPTIONS,
  SETTINGS_CATALOG,
  SETTINGS_ENTRIES,
  attachmentTransferView,
  type SettingsKey,
  type SettingsWidget,
  optionKey,
  withoutAssets,
} from "@/features/settings/settings-catalog";
import {
  ASSET_COPY,
  MESSAGE_KEYS,
  assetDigest,
  assetPreviewSrc,
  isRecord,
  leafValue,
  listFieldValue,
  numberFieldValue,
  previewText,
  settingLabel as label,
  settingMatches as matches,
  textFieldValue,
  withLeaf,
  withMessageOverride,
} from "@/features/settings/settings-instance-model";
import { fieldClass, textareaClass } from "./field-classes";
import "@/features/settings/settings-shell.css";

type AdminInstanceSettingsOutput = components["schemas"]["AdminInstanceSettingsOutput"];

const props = defineProps<{
  data: AdminInstanceSettingsOutput | null;
  loading: boolean;
  error: string | null;
  onRetry?: () => void;
  pending: boolean;
  onSave: (patch: Record<string, unknown>) => Promise<void>;
  onAsset: (kind: BrandingAssetKind, file: File | null) => Promise<void>;
}>();

const query = ref("");
const draft = ref<Partial<Record<SettingsKey, unknown>>>({});
const assetProblems = ref<Partial<Record<BrandingAssetKind, string>>>({});
const saving = ref(false);
const busy = computed(() => props.pending || saving.value);
const saveError = ref<string | null>(null);
const saved = ref(false);

const values = computed<Record<string, unknown>>(() => props.data?.values ?? {});
const overridden = computed(() => new Set(props.data?.overridden ?? []));
const restartPending = computed(() => new Set(props.data?.restartRequired ?? []));
const envApplied = computed(() => new Set(props.data?.envApplied ?? []));
const eeFeatures = computed(() => new Set(props.data?.eeFeatures ?? []));
const transfer = computed(() => attachmentTransferView(props.data?.attachmentTransfer));
const instanceName = computed(() => {
  const brandingName = leafValue(values.value.branding, "name");
  return typeof brandingName === "string" ? brandingName : "";
});
const shown = computed(() =>
  SETTINGS_ENTRIES.filter(([key, entry]) => matches(key, entry, query.value)),
);

function currentValue(key: SettingsKey): unknown {
  return draft.value[key] !== undefined ? draft.value[key] : values.value[key];
}

function edited(key: SettingsKey): boolean {
  return draft.value[key] !== undefined;
}

function issueFor(key: SettingsKey) {
  if (!edited(key)) return undefined;
  const parsed = SETTINGS_CATALOG[key].schema.safeParse(currentValue(key));
  return parsed.success ? undefined : parsed.error.issues[0];
}

function eeLocked(key: SettingsKey): boolean {
  const gate = EE_GATED[key];
  return gate !== undefined && !eeFeatures.value.has(gate);
}

function setLeaf(key: SettingsKey, leaf: string, next: unknown): void {
  draft.value = {
    ...draft.value,
    [key]: withLeaf(draft.value[key] ?? values.value[key], leaf, next),
  };
}

async function submit(key: SettingsKey, next: unknown): Promise<void> {
  if (next === undefined || busy.value || eeLocked(key)) return;
  if (next !== null && !SETTINGS_CATALOG[key].schema.safeParse(next).success) return;
  saving.value = true;
  saveError.value = null;
  saved.value = false;
  try {
    await props.onSave({ [key]: next === null ? null : withoutAssets(key, next) });
  } catch (err) {
    saveError.value = problemMessage(err, "settings.save.failed");
    return;
  } finally {
    saving.value = false;
  }
  draft.value = Object.fromEntries(
    Object.entries(draft.value).filter(([draftKey]) => draftKey !== key),
  );
  saved.value = true;
}

function runAsset(kind: BrandingAssetKind, file: File | null): void {
  if (busy.value) return;
  saving.value = true;
  assetProblems.value = { ...assetProblems.value, [kind]: undefined };
  void props
    .onAsset(kind, file)
    .catch((err: unknown) => {
      assetProblems.value = {
        ...assetProblems.value,
        [kind]: problemMessage(err, "error.network"),
      };
    })
    .finally(() => {
      saving.value = false;
    });
}

function pickAsset(kind: BrandingAssetKind, event: Event): void {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (file !== undefined) runAsset(kind, file);
}

function overrideMap(value: unknown): Record<string, unknown> {
  return isRecord(value) ? value : {};
}

function overrideText(value: unknown, key: string): string {
  const raw = overrideMap(value)[key];
  return typeof raw === "string" ? raw : "";
}

function widgetOf(entry: (typeof SETTINGS_ENTRIES)[number][1], leaf: string): SettingsWidget {
  const widget = entry.widgets[leaf];
  if (widget === undefined) throw new Error(`Missing settings widget: ${leaf}`);
  return widget;
}

function assetKindOf(leaf: string): BrandingAssetKind | null {
  return BRANDING_ASSET_KINDS.find((kind) => kind === leaf) ?? null;
}

function enumOptions(
  key: SettingsKey,
  leaf: string,
): { value: string; label: string; disabled?: boolean }[] {
  return (SETTING_ENUM_OPTIONS[`${key}.${leaf}`] ?? []).map((value) => ({
    value,
    label: label(optionKey(key, leaf, value)),
    disabled: key === "attachmentTransfer" && transfer.value.disabledOptions.has(value),
  }));
}

function numberShown(value: unknown): string | number {
  return typeof value === "number" ? value : "";
}

function textShown(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function listShown(value: unknown): string {
  return Array.isArray(value) ? value.join("\n") : "";
}

const badgeClass =
  "inline-flex items-center rounded-full border border-default px-2 py-0.5 text-xs text-muted";
</script>

<template>
  <section class="settings-section" aria-labelledby="instance-settings-title">
    <h2 id="instance-settings-title" class="settings-section__title text-title">{{
      t("settings.ui.title")
    }}</h2>
    <p class="settings-section__lede">{{ t("settings.ui.help") }}</p>
    <p v-if="saveError" role="alert" class="text-sm text-error">{{ saveError }}</p>
    <p v-if="saved" role="status" class="text-sm text-muted">{{ t("workspace.settings.saved") }}</p>
    <div class="flex flex-col gap-4 text-sm">
      <label class="sr-only" for="settings-search">{{ t("settings.ui.search") }}</label>
      <input
        id="settings-search"
        :class="fieldClass"
        :placeholder="t('settings.ui.search')"
        :value="query"
        @input="query = inputText($event)"
      />
      <QueryLoading v-if="loading" />
      <QueryError v-if="error && onRetry" :message="error" @retry="onRetry" />
      <p v-else-if="error" class="text-sm text-error" role="alert">{{ error }}</p>
      <p v-if="!loading && shown.length === 0" class="text-sm text-muted">{{
        t("settings.ui.noMatch")
      }}</p>
      <template v-if="data !== null">
        <section
          v-for="[key, entry] in shown"
          :key="key"
          :aria-labelledby="`setting-${key}`"
          class="flex flex-col gap-3 rounded-md border border-default p-4"
        >
          <div class="flex flex-wrap items-center gap-2">
            <h3 :id="`setting-${key}`" class="text-sm font-medium">{{ label(entry.labelKey) }}</h3>
            <span :class="badgeClass">{{ label(entry.group) }}</span>
            <span v-if="entry.safety === 'restart_required'" :class="badgeClass">{{
              t("settings.ui.restart")
            }}</span>
            <span v-if="overridden.has(key)" :class="badgeClass">{{
              t("settings.ui.overridden")
            }}</span>
            <span v-if="eeLocked(key)" :class="badgeClass">{{ t("ee.badge") }}</span>
          </div>
          <p class="text-xs text-muted">{{ label(entry.helpKey) }}</p>
          <p v-if="eeLocked(key)" class="text-xs text-muted">{{ t("ee.required") }}</p>
          <p v-if="restartPending.has(key)" class="text-xs text-error">{{
            t("settings.ui.restartPending")
          }}</p>
          <p
            v-if="key === 'attachmentTransfer' && transfer.effectiveOptionKey !== null"
            class="text-xs text-muted"
          >
            {{
              t("settings.attachmentTransfer.effective", {
                mode: label(transfer.effectiveOptionKey),
              })
            }}
          </p>
          <p
            v-if="key === 'attachmentTransfer' && transfer.unavailableKey !== null"
            class="text-xs text-muted"
          >
            {{ t(transfer.unavailableKey) }}
          </p>
          <p
            v-if="key === 'attachmentTransfer' && transfer.blocked"
            class="text-xs text-error"
            role="alert"
          >
            {{ t("settings.attachmentTransfer.blocked") }}
          </p>
          <div v-for="leaf in Object.keys(entry.widgets)" :key="leaf" class="flex flex-col gap-1">
            <label :id="`${key}.${leaf}-label`" class="font-mono text-xs" :for="`${key}.${leaf}`">
              {{ `${key}.${leaf}` }}
            </label>
            <template v-if="widgetOf(entry, leaf) === 'asset'">
              <div v-if="assetKindOf(leaf)" class="flex flex-col gap-2">
                <p
                  v-if="assetDigest(leafValue(values[key], leaf)) === null"
                  class="text-xs text-muted"
                >
                  {{ t(ASSET_COPY[assetKindOf(leaf)!].none) }}
                </p>
                <div
                  v-else
                  class="flex h-16 w-40 items-center justify-start rounded-md border border-default p-2"
                >
                  <img
                    :alt="t(ASSET_COPY[assetKindOf(leaf)!].alt, { name: instanceName })"
                    class="max-h-full max-w-full object-contain"
                    :src="
                      assetPreviewSrc(
                        assetKindOf(leaf)!,
                        assetDigest(leafValue(values[key], leaf))!,
                      )
                    "
                  />
                </div>
                <input
                  :id="`${key}.${leaf}`"
                  :class="fieldClass"
                  type="file"
                  :accept="BRANDING_ASSET_MIME.join(',')"
                  :aria-describedby="
                    assetProblems[assetKindOf(leaf)!] ? `${key}.${leaf}-problem` : undefined
                  "
                  :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
                  @change="pickAsset(assetKindOf(leaf)!, $event)"
                />
                <p
                  v-if="assetProblems[assetKindOf(leaf)!]"
                  :id="`${key}.${leaf}-problem`"
                  class="text-xs text-error"
                  role="alert"
                >
                  {{ assetProblems[assetKindOf(leaf)!] }}
                </p>
                <div v-if="assetDigest(leafValue(values[key], leaf)) !== null">
                  <ConfirmAction
                    :action-label="t(ASSET_COPY[assetKindOf(leaf)!].clear)"
                    :description="t(ASSET_COPY[assetKindOf(leaf)!].body)"
                    :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
                    :on-confirm="() => runAsset(assetKindOf(leaf)!, null)"
                    :title="t(ASSET_COPY[assetKindOf(leaf)!].title)"
                  >
                    {{ t(ASSET_COPY[assetKindOf(leaf)!].clear) }}
                  </ConfirmAction>
                </div>
              </div>
            </template>
            <input
              v-else-if="widgetOf(entry, leaf) === 'boolean'"
              :id="`${key}.${leaf}`"
              type="checkbox"
              class="size-5"
              :checked="leafValue(currentValue(key), leaf) === true"
              :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
              @change="setLeaf(key, leaf, inputChecked($event))"
            />
            <select
              v-else-if="widgetOf(entry, leaf) === 'enum'"
              :id="`${key}.${leaf}`"
              class="h-11 w-56 rounded-md border border-default bg-default px-3 text-sm disabled:opacity-50"
              :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
              :value="String(leafValue(currentValue(key), leaf))"
              @change="
                setLeaf(
                  key,
                  leaf,
                  typeof leafValue(currentValue(key), leaf) === 'number'
                    ? Number(inputText($event))
                    : inputText($event),
                )
              "
            >
              <option
                v-for="option in enumOptions(key, leaf)"
                :key="option.value"
                :value="option.value"
                :disabled="option.disabled"
              >
                {{ option.label }}
              </option>
            </select>
            <input
              v-else-if="widgetOf(entry, leaf) === 'number' || widgetOf(entry, leaf) === 'duration'"
              :id="`${key}.${leaf}`"
              :class="['w-56', fieldClass]"
              type="number"
              :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
              :value="numberShown(leafValue(currentValue(key), leaf))"
              @input="setLeaf(key, leaf, numberFieldValue(inputText($event)))"
            />
            <input
              v-else-if="widgetOf(entry, leaf) === 'text'"
              :id="`${key}.${leaf}`"
              :class="fieldClass"
              :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
              :value="textShown(leafValue(currentValue(key), leaf))"
              @input="setLeaf(key, leaf, textFieldValue(inputText($event)))"
            />
            <textarea
              v-else-if="widgetOf(entry, leaf) === 'list'"
              :id="`${key}.${leaf}`"
              :class="textareaClass"
              rows="4"
              :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
              :value="listShown(leafValue(currentValue(key), leaf))"
              @input="setLeaf(key, leaf, listFieldValue(inputText($event)))"
            />
            <div v-else-if="widgetOf(entry, leaf) === 'i18n_override'" class="flex flex-col gap-4">
              <div v-for="msgKey in MESSAGE_KEYS" :key="msgKey" class="flex flex-col gap-1">
                <label class="font-mono text-xs" :for="`msg-${msgKey}`">{{ msgKey }}</label>
                <p class="whitespace-pre-line text-xs text-muted">{{ label(msgKey) }}</p>
                <textarea
                  :id="`msg-${msgKey}`"
                  :class="textareaClass"
                  :disabled="busy || envApplied.has(`${key}.${leaf}`) || eeLocked(key)"
                  rows="2"
                  :value="overrideText(leafValue(currentValue(key), leaf), msgKey)"
                  @input="
                    setLeaf(
                      key,
                      leaf,
                      withMessageOverride(
                        overrideMap(leafValue(currentValue(key), leaf)),
                        msgKey,
                        inputText($event),
                      ),
                    )
                  "
                />
                <p
                  v-if="overrideText(leafValue(currentValue(key), leaf), msgKey).includes('<')"
                  class="text-xs text-error"
                  role="alert"
                >
                  {{ t("settings.ui.angleBracket") }}
                </p>
                <p
                  v-if="overrideText(leafValue(currentValue(key), leaf), msgKey) !== ''"
                  class="text-xs text-muted"
                >
                  {{ t("settings.ui.preview") }}:
                  {{ previewText(overrideText(leafValue(currentValue(key), leaf), msgKey)) }}
                </p>
              </div>
            </div>
            <p v-if="envApplied.has(`${key}.${leaf}`)" class="text-xs text-muted">{{
              t("settings.ui.envFixed")
            }}</p>
          </div>
          <div class="flex flex-wrap gap-2">
            <ConfirmAction
              v-if="entry.confirmDestructive"
              :action-label="t('settings.ui.save')"
              :description="t('settings.ui.confirmBody')"
              :disabled="busy || !edited(key) || issueFor(key) !== undefined || eeLocked(key)"
              :on-confirm="() => submit(key, draft[key])"
              :title="t('settings.ui.confirmTitle')"
              trigger-variant="default"
            >
              {{ t("settings.ui.save") }}
            </ConfirmAction>
            <UButton
              v-else
              type="button"
              size="sm"
              :disabled="busy || !edited(key) || issueFor(key) !== undefined || eeLocked(key)"
              @click="submit(key, draft[key])"
            >
              {{ t("settings.ui.save") }}
            </UButton>
            <UButton
              v-if="overridden.has(key)"
              type="button"
              size="sm"
              variant="outline"
              color="neutral"
              :disabled="busy || eeLocked(key)"
              @click="submit(key, null)"
            >
              {{ t("settings.ui.reset") }}
            </UButton>
          </div>
          <p v-if="issueFor(key)" class="text-xs text-error" role="alert">
            {{ issueFor(key)!.path.join(".") }}: {{ issueFor(key)!.message }}
          </p>
        </section>
      </template>
    </div>
  </section>
</template>
