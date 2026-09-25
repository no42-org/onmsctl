/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docs: [
    'intro',
    {
      type: 'category',
      label: 'Getting started',
      collapsed: false,
      items: ['getting-started/install', 'getting-started/configure-context', 'getting-started/quickstart'],
    },
    {
      type: 'category',
      label: 'Concepts',
      items: ['concepts/declarative-apply', 'concepts/read-only-contexts'],
    },
    {
      type: 'category',
      label: 'Kinds',
      items: [
        'kinds/event-source',
        'kinds/requisition',
        'kinds/user',
        'kinds/snmp-config',
        'kinds/maintenance',
        'kinds/datacollection-source',
        'kinds/business-service',
      ],
    },
  ],
};

export default sidebars;
