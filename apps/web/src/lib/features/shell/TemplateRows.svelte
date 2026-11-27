<!--
  The templates a new thing can start from, by the kind they make: the
  workspace's own (which can be opened to be changed) and the built-in ones.
-->
<script lang="ts">
  import PencilIcon from '@lucide/svelte/icons/pencil';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Container, ContainerKind } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';

  import ContainerIcon from './ContainerIcon.svelte';

  interface Props {
    templates: readonly Container[];
    locale: string;
    onUse: (template: Container) => void;
    onEdit: (template: Container) => void;
  }

  let { templates, locale, onUse, onEdit }: Props = $props();

  const GROUPS: readonly { kind: ContainerKind; heading: () => string }[] = [
    { kind: 'board', heading: m.new_templates_board },
    { kind: 'course', heading: m.new_templates_course },
    { kind: 'wiki', heading: m.new_templates_wiki },
  ];

  const grouped = $derived(
    GROUPS.map((group) => ({
      ...group,
      list: templates.filter((template) => (template.kind ?? 'board') === group.kind),
    })).filter((group) => group.list.length > 0),
  );

  const titleOf = (template: Container): string =>
    template.title || m['View.NewTemplateDefaultTitle']();
  const keyOf = (template: Container): string =>
    typeof template.properties.templateKey === 'string'
      ? template.properties.templateKey
      : template.id;
</script>

<div class="grid gap-4" data-testid="templates">
  {#each grouped as group (group.kind)}
    <section class="grid gap-1" data-template-kind={group.kind}>
      <h3 class="px-2 text-xs font-medium text-muted-foreground">{group.heading()}</h3>
      <ul class="grid gap-1">
        {#each group.list as template (template.id)}
          {@const title = titleOf(template)}
          <li class="flex items-start gap-2 rounded-md p-2 hover:bg-accent" data-template={title}>
            <ContainerIcon icon={template.icon} kind={template.kind} {title} {locale} />
            <div class="min-w-0 flex-1">
              <p class="truncate font-medium">{title}</p>
              {#if template.description}
                <p class="line-clamp-2 text-xs text-muted-foreground">{template.description}</p>
              {/if}
            </div>
            {#if !template.isBuiltin}
              <IconButton
                label={m.shell_template_edit({ name: title })}
                class="text-muted-foreground"
                onclick={() => {
                  onEdit(template);
                }}
              >
                <PencilIcon />
              </IconButton>
            {/if}
            <Button
              variant="outline"
              size="sm"
              class="shrink-0 roomy:min-h-touch"
              aria-label={m.shell_template_use({ name: title })}
              data-testid="use-template-{keyOf(template)}"
              onclick={() => {
                onUse(template);
              }}
            >
              {m['BoardTemplateSelector.use-this-template']()}
            </Button>
          </li>
        {/each}
      </ul>
    </section>
  {/each}
</div>
