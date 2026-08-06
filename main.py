from ai import ask_ai

print("REI 0.1")

while True:
    prompt = input("You: ")

    if prompt.lower() == "exit":
        print("REI: Goodbye!")
        break

    answer = ask_ai(prompt)

    print(f"REI: {answer}")