#!/usr/bin/env python3
"""Text Analyzer Skill - analyzes text for various statistics."""

import json
import re
import sys
from collections import Counter


def main():
    # Read input from stdin
    input_data = json.loads(sys.stdin.read())
    text = input_data.get("text", "")

    # Basic counts
    char_count = len(text)
    char_count_no_spaces = len(text.replace(" ", "").replace("\n", "").replace("\t", ""))

    # Word analysis
    words = re.findall(r'\b\w+\b', text.lower())
    word_count = len(words)

    # Sentence count (simple heuristic)
    sentences = re.split(r'[.!?]+', text)
    sentence_count = len([s for s in sentences if s.strip()])

    # Paragraph count
    paragraphs = text.split('\n\n')
    paragraph_count = len([p for p in paragraphs if p.strip()])

    # Average word length
    avg_word_length = sum(len(w) for w in words) / max(word_count, 1)

    # Top words (excluding common stop words)
    stop_words = {'the', 'a', 'an', 'is', 'are', 'was', 'were', 'be', 'been',
                  'being', 'have', 'has', 'had', 'do', 'does', 'did', 'will',
                  'would', 'could', 'should', 'may', 'might', 'must', 'shall',
                  'can', 'to', 'of', 'in', 'for', 'on', 'with', 'at', 'by',
                  'from', 'as', 'into', 'through', 'during', 'before', 'after',
                  'above', 'below', 'between', 'under', 'again', 'further',
                  'then', 'once', 'and', 'but', 'or', 'nor', 'so', 'yet',
                  'both', 'each', 'few', 'more', 'most', 'other', 'some',
                  'such', 'no', 'not', 'only', 'own', 'same', 'than', 'too',
                  'very', 'just', 'also', 'now', 'it', 'its', 'this', 'that',
                  'these', 'those', 'i', 'you', 'he', 'she', 'we', 'they'}

    filtered_words = [w for w in words if w not in stop_words and len(w) > 2]
    word_freq = Counter(filtered_words)
    top_words = [{"word": w, "count": c} for w, c in word_freq.most_common(10)]

    result = {
        "word_count": word_count,
        "char_count": char_count,
        "char_count_no_spaces": char_count_no_spaces,
        "sentence_count": sentence_count,
        "paragraph_count": paragraph_count,
        "average_word_length": round(avg_word_length, 2),
        "top_words": top_words
    }

    print(json.dumps(result))


if __name__ == "__main__":
    main()
