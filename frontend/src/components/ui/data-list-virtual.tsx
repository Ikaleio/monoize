/* eslint-disable react-refresh/only-export-components */
import type { Ref } from "react";
import type { ContextProp, ItemProps, ListProps } from "react-virtuoso";

import { DataListBody, DataListRow } from "@/components/ui/data-list";

export interface VirtualDataListContext {
  /** Accessible name of the virtualized list. */
  label: string;
}

// Virtuoso types its list ref as a div; it only reads HTMLElement geometry through it.
function VirtualDataListBody({ ref, context, ...props }: ListProps & ContextProp<VirtualDataListContext>) {
  return <DataListBody {...props} ref={ref as Ref<HTMLUListElement>} aria-label={context.label} />;
}

// Forward only DOM-safe props so Virtuoso's `item` never reaches the <li>.
function VirtualDataListRow(props: ItemProps<unknown>) {
  return (
    <DataListRow
      style={props.style}
      data-index={props["data-index"]}
      data-item-index={props["data-item-index"]}
      data-known-size={props["data-known-size"]}
    >
      {props.children}
    </DataListRow>
  );
}

/** `Virtuoso` components that render a virtualized list through the DataList primitives (DS22k). */
export const virtualDataListComponents = { List: VirtualDataListBody, Item: VirtualDataListRow };
