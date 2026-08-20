/*---
description: test requiring unsupported feature
features: [FinalizationRegistry]
---*/
var fr = new FinalizationRegistry(function() {});
