# Lorem Ipsum Survey

Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor
incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis
nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.

## Background

Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu
fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in
culpa qui officia deserunt mollit anim id est laborum.

> Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium
> doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore
> veritatis et quasi architecto beatae vitae dicta sunt explicabo.

## Instructions

Please fill in all fields below and click **Submit** when done.

- Fields marked with a label are required
- The *environment* and *tier* dropdowns must match your deployment target
- Check the confirmation box only after reviewing the details above

```form
{
  "fields": [
    {
      "name":    "name",
      "type":    "entry",
      "label":   "Your full name",
      "default": "Jane Doe"
    },
    {
      "name":  "password",
      "type":  "password",
      "label": "Session passphrase"
    },
    {
      "name":    "environment",
      "type":    "list",
      "label":   "Target environment",
      "options": ["development", "staging", "production"]
    },
    {
      "name":    "tier",
      "type":    "list",
      "label":   "Service tier",
      "options": ["free", "standard", "enterprise"]
    },
    {
      "name":    "confirmed",
      "type":    "question",
      "label":   "I have read and understood the instructions above"
    }
  ]
}
```
