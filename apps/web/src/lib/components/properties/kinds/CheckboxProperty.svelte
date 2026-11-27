<!--
  The value of a checkbox property. Pressing it changes it at once. Checked
  is stored as `true`, unchecked as an empty value; only `true` reads as
  checked.
-->
<script lang="ts">
  import { Checkbox } from '$lib/ui/checkbox/index.js';

  interface Props {
    /** The name of the property. */
    name: string;
    value: string;
    editable: boolean;
    onChange: (value: string) => void;
  }

  let { name, value, editable, onChange }: Props = $props();
</script>

<label
  data-slot="property-checkbox"
  class="inline-flex min-h-8 min-w-8 items-center px-2 roomy:min-h-touch roomy:min-w-touch"
>
  <Checkbox
    aria-label={name}
    aria-readonly={editable ? undefined : 'true'}
    disabled={!editable}
    bind:checked={
      () => value === 'true',
      (checked) => {
        onChange(checked ? 'true' : '');
      }
    }
    class="disabled:opacity-100"
  />
</label>
