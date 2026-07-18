import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";

import {
  FilterCheckList,
  FilterPanel,
  FilterSearchList,
  FilterSection,
} from "./filter-panel";

const meta = {
  title: "Navigation/FilterPanel",
  component: FilterPanel,
  parameters: { layout: "padded" },
} satisfies Meta<typeof FilterPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

function InteractiveFilterPanel() {
  const [providers, setProviders] = React.useState(["openai"]);
  const [models, setModels] = React.useState<string[]>([]);
  return (
    <FilterPanel title="Invocation filters" className="h-[560px]">
      <FilterSection title="Provider" count={providers.length} defaultOpen>
        <FilterCheckList
          options={[
            { value: "openai", label: "OpenAI" },
            { value: "anthropic", label: "Anthropic" },
            { value: "ollama", label: "Ollama" },
          ]}
          selected={providers}
          onChange={setProviders}
        />
      </FilterSection>
      <FilterSection title="Model" count={models.length} defaultOpen>
        <FilterSearchList
          options={[
            { value: "gpt-4o", label: "gpt-4o" },
            { value: "claude-sonnet", label: "claude-sonnet" },
            { value: "llama-3", label: "llama-3" },
          ]}
          selected={models}
          onChange={setModels}
          placeholder="Find a model…"
        />
      </FilterSection>
    </FilterPanel>
  );
}

export const Default: Story = {
  args: { children: null },
  render: () => <InteractiveFilterPanel />,
};
