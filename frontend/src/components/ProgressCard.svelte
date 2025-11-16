<script lang="ts">
    export let progressPercent = 0;
    export let isCompressing = false;
</script>

<div class="content-card">
    <h2 class="section-title">// processing</h2>
    <div class="progress-container">
        <div class="progress-bar">
            <div 
                class="progress-fill" 
                class:compressing={isCompressing} 
                style={`width: ${progressPercent}%;`}
            ></div>
        </div>
        <div class="progress-text">{progressPercent.toFixed(1)}%</div>
    </div>
</div>

