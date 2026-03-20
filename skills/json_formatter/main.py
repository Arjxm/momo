#!/usr/bin/env python3
"""JSON Formatter Skill - formats and validates JSON strings."""

import json
import sys


def main():
    # Read input from stdin
    input_data = json.loads(sys.stdin.read())

    json_string = input_data.get("json_string", "")
    indent = input_data.get("indent", 2)

    result = {
        "formatted": "",
        "valid": False,
        "error": None
    }

    try:
        # Parse the JSON
        parsed = json.loads(json_string)

        # Format it
        result["formatted"] = json.dumps(parsed, indent=indent, ensure_ascii=False)
        result["valid"] = True

    except json.JSONDecodeError as e:
        result["error"] = f"JSON syntax error at line {e.lineno}, column {e.colno}: {e.msg}"
    except Exception as e:
        result["error"] = str(e)

    # Output result
    print(json.dumps(result))


if __name__ == "__main__":
    main()
